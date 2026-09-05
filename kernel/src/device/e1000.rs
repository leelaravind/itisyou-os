//! Intel 8254x (e1000) Ethernet driver — the NIC QEMU's `pc` machine gives us.
//!
//! Polled, like every other driver here: the receive path is drained from the
//! network service's scheduling slice rather than from an ISR. That is a
//! deliberate continuation of the pattern the storage, audio and USB drivers
//! already follow — a polled driver has no interrupt-context locking rules to
//! get wrong, and the V0.7 PIC path stays untouched. MSI delivery is proved
//! separately (see `interrupts::msi`) without moving the data path into an
//! interrupt handler.
//!
//! Descriptor rings are the whole of the hardware contract:
//!
//! * The card owns a descriptor between the moment we hand it over (by moving
//!   a tail pointer) and the moment it sets the descriptor's DD ("descriptor
//!   done") status bit. Reading a field before DD is reading a race.
//! * Head and tail are the card's and our cursors into the same ring. Tail
//!   must never be advanced onto the head, or the card cannot tell "ring full"
//!   from "ring empty".
//! * Descriptors and buffers are DMA memory: the card reads them with a
//!   physical address and no cache coherency help from us, so every access
//!   goes through `read_volatile`/`write_volatile`.

use super::{Device, Driver, DriverError};
use crate::memory;
use crate::memory::paging;
use crate::serial_println;
use kernel_core::net::eth::MacAddr;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

/// 82540EM — the default `-device e1000` model.
const VENDOR_INTEL: u16 = 0x8086;
const DEVICE_82540EM: u16 = 0x100E;
/// 82574L — `-device e1000e`. Probed too so the driver binds on either model;
/// the register subset used here is common to both.
const DEVICE_82574L: u16 = 0x10D3;

// --- Register offsets (bytes into BAR0) ---
const REG_CTRL: u64 = 0x0000;
const REG_STATUS: u64 = 0x0008;
const REG_ICR: u64 = 0x00C0;
const REG_IMC: u64 = 0x00D8;
const REG_RCTL: u64 = 0x0100;
const REG_TCTL: u64 = 0x0400;
const REG_TIPG: u64 = 0x0410;
const REG_RDBAL: u64 = 0x2800;
const REG_RDBAH: u64 = 0x2804;
const REG_RDLEN: u64 = 0x2808;
const REG_RDH: u64 = 0x2810;
const REG_RDT: u64 = 0x2818;
const REG_TDBAL: u64 = 0x3800;
const REG_TDBAH: u64 = 0x3804;
const REG_TDLEN: u64 = 0x3808;
const REG_TDH: u64 = 0x3810;
const REG_TDT: u64 = 0x3818;
const REG_MTA: u64 = 0x5200;
const REG_RAL0: u64 = 0x5400;
const REG_RAH0: u64 = 0x5404;

// CTRL bits.
const CTRL_SLU: u32 = 1 << 6; // set link up
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;
const CTRL_ILOS: u32 = 1 << 7;
const CTRL_VME: u32 = 1 << 30;

// RCTL bits.
const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15; // accept broadcast
const RCTL_SECRC: u32 = 1 << 26; // strip Ethernet CRC
                                 // BSIZE 00 with BSEX 0 = 2048-byte buffers, which is the default and needs no
                                 // bits set — named here so the choice is visible rather than implied.
const RCTL_BSIZE_2048: u32 = 0;

// TCTL bits.
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3; // pad short packets
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

// Transmit descriptor command/status bits.
const TXD_CMD_EOP: u8 = 1 << 0;
const TXD_CMD_IFCS: u8 = 1 << 1; // insert FCS
const TXD_CMD_RS: u8 = 1 << 3; // report status
const TXD_STAT_DD: u8 = 1 << 0;

// Receive descriptor status/error bits.
const RXD_STAT_DD: u8 = 1 << 0;
const RXD_STAT_EOP: u8 = 1 << 1;

/// Ring depth. A power of two so the wrap is a mask, and small because this
/// stack processes a frame to completion before taking the next one — a deeper
/// ring would only buffer work nobody is doing yet.
const RING_LEN: usize = 16;
const RING_MASK: usize = RING_LEN - 1;
/// Per-descriptor buffer size, matching `RCTL_BSIZE_2048`. Larger than the
/// 1518-byte maximum Ethernet frame, so no frame is ever split across
/// descriptors and the driver never has to reassemble one.
const BUF_SIZE: usize = 2048;
/// Descriptors are 16 bytes in both the receive and transmit layouts.
const DESC_SIZE: usize = 16;

/// A DMA-visible 4 KiB frame.
struct DmaFrame {
    phys: u64,
    virt: u64,
    _frame: PhysFrame,
}

impl DmaFrame {
    fn alloc() -> Option<DmaFrame> {
        let frame = memory::alloc_frame().ok()?;
        let phys = frame.start_address().as_u64();
        let virt = paging::phys_to_virt(phys) as u64;
        // SAFETY: a freshly allocated, exclusively owned frame. Zeroing before
        // any DMA use means the card never sees stale descriptors.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096) };
        Some(DmaFrame {
            phys,
            virt,
            _frame: frame,
        })
    }
}

/// Statistics the shell and the tests can read back. Counting refusals
/// separately from successes is what makes a silent drop impossible to
/// mistake for an idle link.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub tx_frames: u64,
    pub tx_bytes: u64,
    pub tx_dropped: u64,
    pub rx_frames: u64,
    pub rx_bytes: u64,
    /// Frames the card marked as spanning descriptors or erroring, which this
    /// driver drops rather than reassembling.
    pub rx_dropped: u64,
}

pub struct E1000 {
    bar: u64,
    mac: MacAddr,
    rx_ring: DmaFrame,
    tx_ring: DmaFrame,
    /// Backing frames for the packet buffers; held so they are never reused.
    _buffers: [DmaFrame; RING_LEN],
    rx_bufs: [(u64, u64); RING_LEN], // (phys, virt)
    tx_bufs: [(u64, u64); RING_LEN],
    rx_cur: usize,
    tx_cur: usize,
    stats: Stats,
}

// Accessed only under STATE's mutex.
unsafe impl Send for E1000 {}

impl E1000 {
    fn read32(&self, off: u64) -> u32 {
        // SAFETY: `off` is a register inside the mapped BAR window; MMIO must
        // not be cached or reordered, hence volatile.
        unsafe { core::ptr::read_volatile((self.bar + off) as *const u32) }
    }

    fn write32(&self, off: u64, val: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.bar + off) as *mut u32, val) };
    }

    fn rx_desc(&self, i: usize) -> u64 {
        self.rx_ring.virt + (i * DESC_SIZE) as u64
    }

    fn tx_desc(&self, i: usize) -> u64 {
        self.tx_ring.virt + (i * DESC_SIZE) as u64
    }

    pub fn mac(&self) -> MacAddr {
        self.mac
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Is the link reported up? `STATUS.LU` is set by the emulated PHY as soon
    /// as SLU is asserted, so this is a real read, not an assumption.
    pub fn link_up(&self) -> bool {
        self.read32(REG_STATUS) & (1 << 1) != 0
    }

    /// Transmit one Ethernet frame. Returns false when the ring is full — the
    /// caller decides whether to retry, and the drop is counted either way.
    pub fn transmit(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > BUF_SIZE {
            self.stats.tx_dropped += 1;
            return false;
        }
        let idx = self.tx_cur;
        let desc = self.tx_desc(idx);
        // SAFETY: descriptor inside our own ring frame; the card writes only
        // the status byte, which we read volatile.
        let status = unsafe { core::ptr::read_volatile((desc + 12) as *const u8) };
        // A descriptor we previously handed over is reusable once the card has
        // reported DD. Index 0 on the very first pass has never been used, and
        // its status byte is zero from the frame being zeroed at allocation —
        // handled by tracking whether we have wrapped at least once.
        if self.tx_cur_in_flight(idx) && status & TXD_STAT_DD == 0 {
            self.stats.tx_dropped += 1;
            return false;
        }
        let (phys, virt) = self.tx_bufs[idx];
        // SAFETY: our own DMA buffer, at least BUF_SIZE bytes, not currently
        // owned by the card (checked above).
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), virt as *mut u8, frame.len());
            core::ptr::write_volatile(desc as *mut u64, phys);
            core::ptr::write_volatile((desc + 8) as *mut u16, frame.len() as u16);
            core::ptr::write_volatile((desc + 10) as *mut u8, 0); // CSO
            core::ptr::write_volatile(
                (desc + 11) as *mut u8,
                TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS,
            );
            core::ptr::write_volatile((desc + 12) as *mut u8, 0); // clear DD
            core::ptr::write_volatile((desc + 13) as *mut u8, 0); // CSS
            core::ptr::write_volatile((desc + 14) as *mut u16, 0); // special
        }
        self.tx_cur = (idx + 1) & RING_MASK;
        self.write32(REG_TDT, self.tx_cur as u32);
        self.stats.tx_frames += 1;
        self.stats.tx_bytes += frame.len() as u64;
        true
    }

    /// Has descriptor `idx` been handed to the card at least once? Before the
    /// first wrap it has not, so its zero status must not be read as "still
    /// owned by the card".
    fn tx_cur_in_flight(&self, idx: usize) -> bool {
        self.stats.tx_frames as usize > idx
    }

    /// Take the next completed received frame, if any, copying it into `out`.
    /// Returns the frame length.
    ///
    /// Copying rather than lending the DMA buffer is deliberate: the
    /// descriptor is returned to the card immediately, so a borrowed slice
    /// would alias memory the card may overwrite at any moment.
    pub fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        let idx = self.rx_cur;
        let desc = self.rx_desc(idx);
        // SAFETY: descriptor inside our own ring frame.
        let (len, status, errors) = unsafe {
            (
                core::ptr::read_volatile((desc + 8) as *const u16) as usize,
                core::ptr::read_volatile((desc + 12) as *const u8),
                core::ptr::read_volatile((desc + 13) as *const u8),
            )
        };
        if status & RXD_STAT_DD == 0 {
            return None;
        }
        let mut result = None;
        // EOP clear means the frame spanned descriptors, which cannot happen
        // with 2048-byte buffers and a 1518-byte MTU; treat it as corruption
        // rather than trying to reassemble. Any error bit is likewise fatal
        // for this frame.
        if status & RXD_STAT_EOP != 0 && errors == 0 && len > 0 && len <= BUF_SIZE {
            let n = core::cmp::min(len, out.len());
            let (_, virt) = self.rx_bufs[idx];
            // SAFETY: our own DMA buffer; the card has finished with it (DD).
            unsafe { core::ptr::copy_nonoverlapping(virt as *const u8, out.as_mut_ptr(), n) };
            self.stats.rx_frames += 1;
            self.stats.rx_bytes += len as u64;
            result = Some(n);
        } else {
            self.stats.rx_dropped += 1;
        }
        // Hand the descriptor back: clear status, then move the tail onto it.
        // Order matters — the card may consume the descriptor the instant the
        // tail moves.
        // SAFETY: descriptor inside our own ring frame.
        unsafe { core::ptr::write_volatile((desc + 12) as *mut u8, 0) };
        self.write32(REG_RDT, idx as u32);
        self.rx_cur = (idx + 1) & RING_MASK;
        result
    }

    /// Read the MAC the card was configured with.
    ///
    /// QEMU pre-loads receive address register 0 from the EEPROM, so this is
    /// the address the outside world will use for us. A zero RAH/RAL would
    /// mean an uninitialized card, which is worth failing on rather than
    /// transmitting from 00:00:00:00:00:00.
    fn read_mac(&self) -> Option<MacAddr> {
        let low = self.read32(REG_RAL0);
        let high = self.read32(REG_RAH0);
        let mac = MacAddr([
            low as u8,
            (low >> 8) as u8,
            (low >> 16) as u8,
            (low >> 24) as u8,
            high as u8,
            (high >> 8) as u8,
        ]);
        if mac.is_zero() {
            None
        } else {
            Some(mac)
        }
    }
}

pub struct E1000Driver;
pub static E1000_DRIVER: E1000Driver = E1000Driver;

/// The bound controller. `None` until a matching device attaches.
static STATE: Mutex<Option<E1000>> = Mutex::new(None);

/// Run `f` over the controller, if one is present.
pub fn with<R>(f: impl FnOnce(&mut E1000) -> R) -> Option<R> {
    STATE.lock().as_mut().map(f)
}

/// Is a NIC bound?
pub fn present() -> bool {
    STATE.lock().is_some()
}

impl Driver for E1000Driver {
    fn name(&self) -> &'static str {
        "e1000"
    }

    fn probe(&self, dev: &Device) -> bool {
        let id = dev.id();
        id.vendor == VENDOR_INTEL && (id.device == DEVICE_82540EM || id.device == DEVICE_82574L)
    }

    fn attach(&self, dev: &Device) -> Result<(), DriverError> {
        dev.pci
            .enable_command_bits(super::pci::CMD_MEM_SPACE | super::pci::CMD_BUS_MASTER);
        let (phys, _) = dev
            .first_mem_bar()
            .ok_or(DriverError::InitFailed("e1000 needs a memory BAR"))?;
        // The register file is 128 KiB; only the first 24 KiB is touched here,
        // but mapping the whole window keeps every offset valid.
        let bar = paging::map_mmio(phys, 0x20000)
            .map_err(|_| DriverError::InitFailed("e1000 BAR map failed"))?;

        let rx_ring = DmaFrame::alloc().ok_or(DriverError::InitFailed("e1000 rx ring"))?;
        let tx_ring = DmaFrame::alloc().ok_or(DriverError::InitFailed("e1000 tx ring"))?;
        // Two 2048-byte buffers per 4 KiB frame; RING_LEN receive plus
        // RING_LEN transmit buffers therefore need RING_LEN frames.
        let buffers = heapless_frames()?;
        let mut rx_bufs = [(0u64, 0u64); RING_LEN];
        let mut tx_bufs = [(0u64, 0u64); RING_LEN];
        for i in 0..RING_LEN {
            let f = &buffers[i];
            rx_bufs[i] = (f.phys, f.virt);
            tx_bufs[i] = (f.phys + BUF_SIZE as u64, f.virt + BUF_SIZE as u64);
        }

        let mut nic = E1000 {
            bar,
            mac: MacAddr::ZERO,
            rx_ring,
            tx_ring,
            rx_bufs,
            tx_bufs,
            rx_cur: 0,
            tx_cur: 0,
            stats: Stats::default(),
            _buffers: buffers,
        };

        // 1. Mask every interrupt source before touching anything else: the
        //    data path is polled, and an unexpected line-level interrupt from
        //    a device with no handler would be a hang, not a diagnostic.
        nic.write32(REG_IMC, 0xFFFF_FFFF);
        let _ = nic.read32(REG_ICR); // clear any latched cause

        // 2. Reset, then mask again — a reset re-arms the mask register.
        nic.write32(REG_CTRL, nic.read32(REG_CTRL) | CTRL_RST);
        let mut deadline = crate::interrupts::Deadline::after_ms(100);
        while nic.read32(REG_CTRL) & CTRL_RST != 0 && deadline.pending() {}
        nic.write32(REG_IMC, 0xFFFF_FFFF);
        let _ = nic.read32(REG_ICR);

        // 3. Link up, no loopback, no VLAN tagging (the Ethernet parser
        //    refuses VLAN frames, so the card must not add tags either).
        let ctrl = nic.read32(REG_CTRL);
        nic.write32(
            REG_CTRL,
            (ctrl | CTRL_SLU) & !(CTRL_ILOS | CTRL_VME | CTRL_PHY_RST),
        );

        nic.mac = nic
            .read_mac()
            .ok_or(DriverError::InitFailed("e1000 has no station address"))?;

        // 4. Clear the multicast table filter: after reset it is undefined,
        //    and a stale entry would silently accept traffic for a group we
        //    never joined.
        for i in 0..128u64 {
            nic.write32(REG_MTA + i * 4, 0);
        }

        // 5. Receive ring. Descriptors are laid out in the ring frame; the
        //    tail starts at the last descriptor so all RING_LEN are owned by
        //    the card and none is both owned and pointed at by the tail.
        for i in 0..RING_LEN {
            let desc = nic.rx_desc(i);
            let (phys, _) = nic.rx_bufs[i];
            // SAFETY: our own ring frame, card not yet enabled.
            unsafe {
                core::ptr::write_volatile(desc as *mut u64, phys);
                core::ptr::write_volatile((desc + 8) as *mut u64, 0);
            }
        }
        nic.write32(REG_RDBAL, nic.rx_ring.phys as u32);
        nic.write32(REG_RDBAH, (nic.rx_ring.phys >> 32) as u32);
        nic.write32(REG_RDLEN, (RING_LEN * DESC_SIZE) as u32);
        nic.write32(REG_RDH, 0);
        nic.write32(REG_RDT, (RING_LEN - 1) as u32);
        // Promiscuous modes (RCTL.UPE/MPE) are deliberately NOT enabled: the
        // card filters to our own address plus broadcast, so the stack above
        // never has to decide whether a frame addressed elsewhere was meant
        // for it. The IP layer re-checks anyway, which is what the harness's
        // "ping addressed elsewhere" probe exercises.
        nic.write32(REG_RCTL, RCTL_EN | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC);

        // 6. Transmit ring.
        for i in 0..RING_LEN {
            let desc = nic.tx_desc(i);
            // SAFETY: our own ring frame.
            unsafe {
                core::ptr::write_volatile(desc as *mut u64, 0);
                core::ptr::write_volatile((desc + 8) as *mut u64, 0);
            }
        }
        nic.write32(REG_TDBAL, nic.tx_ring.phys as u32);
        nic.write32(REG_TDBAH, (nic.tx_ring.phys >> 32) as u32);
        nic.write32(REG_TDLEN, (RING_LEN * DESC_SIZE) as u32);
        nic.write32(REG_TDH, 0);
        nic.write32(REG_TDT, 0);
        // Collision threshold 15 and collision distance 64 are the values the
        // datasheet specifies for full duplex.
        nic.write32(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT),
        );
        nic.write32(REG_TIPG, 0x0060_200A);

        let mut mac_buf = [0u8; 17];
        serial_println!(
            "[ITISYOU:NET] nic_ready driver=e1000 mac={} link_up={} rx_ring={} tx_ring={}",
            nic.mac.format(&mut mac_buf),
            nic.link_up(),
            RING_LEN,
            RING_LEN,
        );
        *STATE.lock() = Some(nic);
        Ok(())
    }
}

/// Allocate the fixed-size buffer-frame array.
///
/// Written as a helper because `[DmaFrame; RING_LEN]` cannot be built from a
/// fallible expression with array syntax, and `DmaFrame` is deliberately not
/// `Copy` (each one owns a physical frame).
fn heapless_frames() -> Result<[DmaFrame; RING_LEN], DriverError> {
    let mut v = alloc::vec::Vec::with_capacity(RING_LEN);
    for _ in 0..RING_LEN {
        v.push(DmaFrame::alloc().ok_or(DriverError::InitFailed("e1000 packet buffers"))?);
    }
    // Length is exactly RING_LEN by construction.
    match <[DmaFrame; RING_LEN]>::try_from(v) {
        Ok(a) => Ok(a),
        Err(_) => Err(DriverError::InitFailed("e1000 packet buffers")),
    }
}
