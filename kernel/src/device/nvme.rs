//! Minimal read-only NVMe driver (V0.3, ADR-0008).
//!
//! Brings up a QEMU-emulated NVMe controller far enough to read blocks:
//! reset, admin queue, Identify (namespace block count), one I/O queue pair,
//! and the Read command. Polled completions (no MSI-X), single namespace,
//! read-only — deliberately the smallest thing that proves real block I/O.
//!
//! DMA buffers come from the PMM (single 4 KiB frames, physically
//! contiguous); the controller sees their physical addresses while the
//! kernel accesses them through the physical-memory alias.

use super::block::{BlockDevice, BlockError, BLOCK_SIZE};
use super::pci::PciDevice;
use crate::memory::{self, paging};
use core::cell::UnsafeCell;
use core::sync::atomic::{compiler_fence, Ordering};
use kernel_core::pci::decode_bar;
use x86_64::structures::paging::PhysFrame;

// Controller register offsets (from BAR0).
const REG_CAP: u64 = 0x00;
const REG_CC: u64 = 0x14;
const REG_CSTS: u64 = 0x1C;
const REG_AQA: u64 = 0x24;
const REG_ASQ: u64 = 0x28;
const REG_ACQ: u64 = 0x30;
const DOORBELL_BASE: u64 = 0x1000;

const QDEPTH: u16 = 8;

#[derive(Debug)]
pub enum NvmeError {
    NotMmioBar,
    Map(paging::PagingError),
    Pmm(memory::PmmError),
    ControllerTimeout,
    AdminFailed { status: u16 },
    IdentifyFailed,
    NoNamespace,
}

struct Dma {
    phys: u64,
    virt: u64,
    frame: PhysFrame,
}

impl Dma {
    fn alloc() -> Result<Dma, NvmeError> {
        let frame = memory::alloc_frame().map_err(NvmeError::Pmm)?;
        let phys = frame.start_address().as_u64();
        let virt = paging::phys_to_virt(phys) as u64;
        // SAFETY: fresh exclusively-owned frame; zero it before DMA use.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096) };
        Ok(Dma { phys, virt, frame })
    }
}

/// A submission/completion queue pair sharing one doorbell stride.
struct QueuePair {
    sq: Dma,
    cq: Dma,
    sq_tail: u16,
    cq_head: u16,
    phase: bool,
    qid: u16,
}

pub struct Nvme {
    bar: u64,
    dstrd: u64,
    // Queue cursors mutate during read_block(&self); the driver is only
    // used single-threaded from the selftest path (no aliasing).
    queues: UnsafeCell<Queues>,
    data: Dma,
    blocks: u64,
    lba_bytes: u64,
}

struct Queues {
    admin: QueuePair,
    io: QueuePair,
}

// Single CPU, single-threaded storage use.
unsafe impl Sync for Nvme {}

impl Nvme {
    fn mmio_read32(&self, off: u64) -> u32 {
        // SAFETY: off within the mapped BAR window; uncacheable MMIO.
        unsafe { core::ptr::read_volatile((self.bar + off) as *const u32) }
    }
    fn mmio_write32(&self, off: u64, val: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.bar + off) as *mut u32, val) };
    }
    fn mmio_read64(&self, off: u64) -> u64 {
        (self.mmio_read32(off) as u64) | ((self.mmio_read32(off + 4) as u64) << 32)
    }
    fn mmio_write64(&self, off: u64, val: u64) {
        self.mmio_write32(off, val as u32);
        self.mmio_write32(off + 4, (val >> 32) as u32);
    }

    fn doorbell(&self, qid: u16, is_cq: bool) -> u64 {
        let stride = 4u64 << self.dstrd;
        DOORBELL_BASE + (2 * qid as u64 + is_cq as u64) * stride
    }

    /// Submit a 16-dword command on the given queue and poll its completion.
    /// Returns the completion status field (0 == success).
    fn submit(&self, admin: bool, cmd: &[u32; 16]) -> u16 {
        // SAFETY: single-threaded storage use; no other reference to the
        // queues exists while this runs.
        let queues = unsafe { &mut *self.queues.get() };
        let q = if admin {
            &mut queues.admin
        } else {
            &mut queues.io
        };
        let slot = q.sq_tail as u64;
        let entry = q.sq.virt + slot * 64;
        for (i, &dw) in cmd.iter().enumerate() {
            // SAFETY: entry within the SQ frame; slot < QDEPTH.
            unsafe { core::ptr::write_volatile((entry + i as u64 * 4) as *mut u32, dw) };
        }
        compiler_fence(Ordering::SeqCst);
        q.sq_tail = (q.sq_tail + 1) % QDEPTH;
        let qid = q.qid;
        self.mmio_write32(self.doorbell(qid, false), q.sq_tail as u32);

        let cqe = q.cq.virt + q.cq_head as u64 * 16;
        let phase = q.phase;
        let mut spins: u64 = 0;
        loop {
            compiler_fence(Ordering::SeqCst);
            // SAFETY: cqe within the CQ frame.
            let dw3 = unsafe { core::ptr::read_volatile((cqe + 12) as *const u32) };
            let cqe_phase = (dw3 >> 16) & 1 == 1;
            if cqe_phase == phase {
                let status = (dw3 >> 17) as u16 & 0x7FF;
                q.cq_head = (q.cq_head + 1) % QDEPTH;
                if q.cq_head == 0 {
                    q.phase = !q.phase;
                }
                self.mmio_write32(self.doorbell(qid, true), q.cq_head as u32);
                return status;
            }
            spins += 1;
            if spins > 20_000_000 {
                return 0xFFFF; // timeout sentinel
            }
            core::hint::spin_loop();
        }
    }

    /// Initialize the controller from a PCI device.
    pub fn init(dev: &PciDevice) -> Result<Nvme, NvmeError> {
        // BAR0 (+BAR1 for the high dword of a 64-bit BAR).
        let bar0 = dev.read_config(0x10);
        let (base_lo, is_mmio, is_64) = decode_bar(bar0);
        if !is_mmio {
            return Err(NvmeError::NotMmioBar);
        }
        let phys = if is_64 {
            base_lo | ((dev.read_config(0x14) as u64) << 32)
        } else {
            base_lo
        };
        // Enable MMIO + bus-master in the command register.
        let cmd = dev.read_config(0x04);
        write_config(dev, 0x04, cmd | 0x6);

        let bar = paging::map_mmio(phys, 0x2000).map_err(NvmeError::Map)?;

        let mut nvme = Nvme {
            bar,
            dstrd: 0,
            queues: UnsafeCell::new(Queues {
                admin: QueuePair {
                    sq: Dma::alloc()?,
                    cq: Dma::alloc()?,
                    sq_tail: 0,
                    cq_head: 0,
                    phase: true,
                    qid: 0,
                },
                io: QueuePair {
                    sq: Dma::alloc()?,
                    cq: Dma::alloc()?,
                    sq_tail: 0,
                    cq_head: 0,
                    phase: true,
                    qid: 1,
                },
            }),
            data: Dma::alloc()?,
            blocks: 0,
            lba_bytes: BLOCK_SIZE as u64,
        };

        let cap = nvme.mmio_read64(REG_CAP);
        nvme.dstrd = (cap >> 32) & 0xF;

        // Disable, wait not-ready.
        nvme.mmio_write32(REG_CC, 0);
        nvme.wait_ready(false)?;

        // Admin queue attributes + base addresses.
        let (asq_phys, acq_phys) = {
            let q = unsafe { &*nvme.queues.get() };
            (q.admin.sq.phys, q.admin.cq.phys)
        };
        nvme.mmio_write32(REG_AQA, ((QDEPTH as u32 - 1) << 16) | (QDEPTH as u32 - 1));
        nvme.mmio_write64(REG_ASQ, asq_phys);
        nvme.mmio_write64(REG_ACQ, acq_phys);

        // Enable: IOSQES=6 (64B), IOCQES=4 (16B), EN=1.
        nvme.mmio_write32(REG_CC, (6 << 16) | (4 << 20) | 1);
        nvme.wait_ready(true)?;

        nvme.identify_namespace()?;
        nvme.create_io_queues()?;
        Ok(nvme)
    }

    fn wait_ready(&self, want: bool) -> Result<(), NvmeError> {
        let mut spins: u64 = 0;
        loop {
            let rdy = self.mmio_read32(REG_CSTS) & 1 == 1;
            if rdy == want {
                return Ok(());
            }
            spins += 1;
            if spins > 20_000_000 {
                return Err(NvmeError::ControllerTimeout);
            }
            core::hint::spin_loop();
        }
    }

    fn identify_namespace(&mut self) -> Result<(), NvmeError> {
        #![allow(clippy::needless_range_loop)]
        // Identify (opcode 0x06), CNS=0 (namespace 1) into the data buffer.
        let mut cmd = [0u32; 16];
        cmd[0] = 0x06 | (0x01 << 16); // opcode | cid=1
        cmd[1] = 1; // NSID
        cmd[6] = self.data.phys as u32; // PRP1 low
        cmd[7] = (self.data.phys >> 32) as u32; // PRP1 high
        cmd[10] = 0; // CNS = 0 (identify namespace)
        let status = self.submit(true, &cmd);
        if status != 0 {
            return Err(NvmeError::AdminFailed { status });
        }
        // Identify Namespace: NSZE (u64) at offset 0, LBAF/FLBAS at 26, LBA
        // format table at 128 (each 4 bytes; LBADS = bits 16:23 = log2 size).
        // SAFETY: data buffer filled by the controller.
        let nsze = unsafe { core::ptr::read_volatile(self.data.virt as *const u64) };
        let flbas = unsafe { core::ptr::read_volatile((self.data.virt + 26) as *const u8) };
        let lbaf_index = (flbas & 0x0F) as u64;
        let lbaf = unsafe {
            core::ptr::read_volatile((self.data.virt + 128 + lbaf_index * 4) as *const u32)
        };
        let lbads = (lbaf >> 16) & 0xFF;
        if nsze == 0 {
            return Err(NvmeError::NoNamespace);
        }
        self.blocks = nsze;
        self.lba_bytes = 1u64 << lbads;
        if self.lba_bytes == 0 {
            return Err(NvmeError::IdentifyFailed);
        }
        Ok(())
    }

    fn create_io_queues(&mut self) -> Result<(), NvmeError> {
        let (io_cq_phys, io_sq_phys) = {
            let q = unsafe { &*self.queues.get() };
            (q.io.cq.phys, q.io.sq.phys)
        };
        // Create I/O Completion Queue (opcode 0x05), qid 1, contiguous.
        //
        // Interrupts are ENABLED on this queue (IEN, bit 1) with interrupt
        // vector 0, so a completion raises MSI-X entry 0 when the platform has
        // armed it. The driver still polls for completions — the interrupt is
        // evidence that message-signalled delivery works, not the mechanism
        // the data path depends on (ADR-0017). With MSI-X unarmed the
        // controller has nowhere to send the message and nothing changes.
        let mut cmd = [0u32; 16];
        cmd[0] = 0x05 | (0x02 << 16);
        cmd[6] = io_cq_phys as u32;
        cmd[7] = (io_cq_phys >> 32) as u32;
        cmd[10] = ((QDEPTH as u32 - 1) << 16) | 1; // qsize | qid
        cmd[11] = 1 | (1 << 1); // physically contiguous | interrupts enabled
        let status = self.submit(true, &cmd);
        if status != 0 {
            return Err(NvmeError::AdminFailed { status });
        }

        // Create I/O Submission Queue (opcode 0x01), qid 1, cqid 1.
        let mut cmd = [0u32; 16];
        cmd[0] = 0x01 | (0x03 << 16);
        cmd[6] = io_sq_phys as u32;
        cmd[7] = (io_sq_phys >> 32) as u32;
        cmd[10] = ((QDEPTH as u32 - 1) << 16) | 1;
        cmd[11] = (1 << 16) | 1; // cqid=1 | physically contiguous
        let status = self.submit(true, &cmd);
        if status != 0 {
            return Err(NvmeError::AdminFailed { status });
        }
        Ok(())
    }
}

impl BlockDevice for Nvme {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        if lba >= self.blocks {
            return Err(BlockError::OutOfRange {
                lba,
                blocks: self.blocks,
            });
        }
        // Queue cursors are interior-mutable (UnsafeCell); single-threaded.
        let mut cmd = [0u32; 16];
        cmd[0] = 0x02 | (0x10 << 16); // Read | cid
        cmd[1] = 1; // NSID
        cmd[6] = self.data.phys as u32;
        cmd[7] = (self.data.phys >> 32) as u32;
        cmd[10] = lba as u32;
        cmd[11] = (lba >> 32) as u32;
        cmd[12] = 0; // read 1 block (0-based)
        let status = self.submit(false, &cmd);
        if status != 0 {
            return Err(BlockError::DeviceError);
        }
        // Copy the (512-byte) block out of the DMA buffer.
        // SAFETY: controller wrote lba_bytes into the data buffer.
        let src = unsafe { core::slice::from_raw_parts(self.data.virt as *const u8, BLOCK_SIZE) };
        buf.copy_from_slice(src);
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        if lba >= self.blocks {
            return Err(BlockError::OutOfRange {
                lba,
                blocks: self.blocks,
            });
        }
        // Stage the block into the DMA buffer.
        // SAFETY: data buffer is an exclusively-owned frame; buf is one block.
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), self.data.virt as *mut u8, BLOCK_SIZE);
        }
        let mut cmd = [0u32; 16];
        cmd[0] = 0x01 | (0x11 << 16); // Write | cid
        cmd[1] = 1; // NSID
        cmd[6] = self.data.phys as u32;
        cmd[7] = (self.data.phys >> 32) as u32;
        cmd[10] = lba as u32;
        cmd[11] = (lba >> 32) as u32;
        cmd[12] = 0; // write 1 block (0-based)
        let status = self.submit(false, &cmd);
        if status != 0 {
            return Err(BlockError::DeviceError);
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        // NVMe Flush (opcode 0x00) commits the volatile write cache.
        let mut cmd = [0u32; 16];
        cmd[0] = 0x12 << 16; // opcode 0x00 (Flush) | cid 0x12
        cmd[1] = 1; // NSID
        let status = self.submit(false, &cmd);
        if status != 0 {
            return Err(BlockError::DeviceError);
        }
        Ok(())
    }
}

impl Drop for Nvme {
    fn drop(&mut self) {
        // Disable the controller and return DMA frames on teardown.
        self.mmio_write32(REG_CC, 0);
        let q = unsafe { &*self.queues.get() };
        for f in [
            q.admin.sq.frame,
            q.admin.cq.frame,
            q.io.sq.frame,
            q.io.cq.frame,
            self.data.frame,
        ] {
            let _ = memory::free_frame(f);
        }
    }
}

fn write_config(dev: &PciDevice, offset: u8, value: u32) {
    use x86_64::instructions::port::Port;
    let addr = 0x8000_0000
        | ((dev.bus as u32) << 16)
        | ((dev.slot as u32) << 11)
        | ((dev.func as u32) << 8)
        | ((offset as u32) & 0xFC);
    // SAFETY: architectural PCI config write mechanism.
    unsafe {
        let mut a = Port::<u32>::new(0xCF8);
        let mut d = Port::<u32>::new(0xCFC);
        a.write(addr);
        d.write(value);
    }
}
