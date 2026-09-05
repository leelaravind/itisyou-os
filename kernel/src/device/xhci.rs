//! xHCI (USB 3.x host controller) — enumeration and HID input.
//!
//! The V0.6 USB path is UHCI, and it works. xHCI is a different machine: where
//! UHCI has the driver build transfer descriptors into a frame list the
//! controller walks, xHCI is ring-based and command-driven. The driver posts
//! TRBs to rings, rings a doorbell, and reads results from an event ring the
//! controller owns. Nothing is polled by walking a schedule; everything is a
//! request and a completion.
//!
//! That shape is why this driver is structured around three rings:
//!
//! * the **command ring**, for controller-level operations (enable a slot,
//!   address a device);
//! * a per-endpoint **transfer ring**, for control and interrupt transfers;
//! * the **event ring**, which the controller writes and the driver reads —
//!   the only place a result ever appears.
//!
//! Like every other driver here it is polled: the event ring is drained with a
//! real-time deadline rather than from an interrupt handler. The reasons are
//! the same as for the NIC (ADR-0015), and the consequence is the same — a
//! missing completion is a timeout with a diagnosable message, never a hang.

use super::{Device, Driver, DriverError};
use crate::memory;
use crate::memory::paging;
use crate::serial_println;
use kernel_core::pci::Bar;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

// --- capability registers (offset 0 of BAR0) --------------------------------
const CAP_CAPLENGTH: u64 = 0x00;
const CAP_HCSPARAMS1: u64 = 0x04;
const CAP_HCSPARAMS2: u64 = 0x08;
const CAP_HCCPARAMS1: u64 = 0x10;
const CAP_DBOFF: u64 = 0x14;
const CAP_RTSOFF: u64 = 0x18;

// --- operational registers (offset CAPLENGTH) -------------------------------
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const OP_CRCR: u64 = 0x18;
const OP_DCBAAP: u64 = 0x30;
const OP_CONFIG: u64 = 0x38;
const OP_PORTSC_BASE: u64 = 0x400;
const OP_PORT_STRIDE: u64 = 0x10;

const USBCMD_RS: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_CNR: u32 = 1 << 11;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
/// Bits that are write-1-to-clear; preserved as zero on every read-modify-write
/// so a status update never clears a change bit the driver did not handle.
const PORTSC_RW1CS: u32 = 0x00FE_0002;

// --- runtime registers (offset RTSOFF) --------------------------------------
const RT_IR0: u64 = 0x20;
const IR_ERSTSZ: u64 = 0x08;
const IR_ERSTBA: u64 = 0x10;
const IR_ERDP: u64 = 0x18;

/// TRB types used here.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_COMPLETION: u32 = 33;
const TRB_PORT_STATUS_CHANGE: u32 = 34;

const TRB_CYCLE: u32 = 1 << 0;
const TRB_ENT: u32 = 1 << 1;
const TRB_ISP: u32 = 1 << 2;
const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6;
const TRB_TOGGLE_CYCLE: u32 = 1 << 1;

/// Entries per ring. Small: this driver runs one transfer at a time.
const RING_LEN: usize = 16;
const TRB_BYTES: usize = 16;

/// A 4 KiB DMA frame.
struct Dma {
    phys: u64,
    virt: u64,
    _frame: PhysFrame,
}

impl Dma {
    fn alloc() -> Option<Dma> {
        let frame = memory::alloc_frame().ok()?;
        let phys = frame.start_address().as_u64();
        let virt = paging::phys_to_virt(phys) as u64;
        // SAFETY: freshly allocated, exclusively owned frame; zeroed before
        // the controller can ever see it.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096) };
        Some(Dma {
            phys,
            virt,
            _frame: frame,
        })
    }
}

/// A TRB ring the driver produces into.
struct Ring {
    dma: Dma,
    index: usize,
    cycle: u32,
}

impl Ring {
    fn new() -> Option<Ring> {
        let dma = Dma::alloc()?;
        // The last TRB is a Link back to the start, with the Toggle Cycle bit
        // so the producer cycle state flips on every lap. Without it the
        // controller would stop as soon as it wrapped.
        let link = dma.virt + ((RING_LEN - 1) * TRB_BYTES) as u64;
        // SAFETY: inside our own ring frame.
        unsafe {
            core::ptr::write_volatile(link as *mut u64, dma.phys);
            core::ptr::write_volatile((link + 8) as *mut u32, 0);
            core::ptr::write_volatile(
                (link + 12) as *mut u32,
                (TRB_LINK << 10) | TRB_TOGGLE_CYCLE | 1,
            );
        }
        Some(Ring {
            dma,
            index: 0,
            cycle: 1,
        })
    }

    /// Write one TRB and advance, handling the wrap through the Link TRB.
    fn push(&mut self, p0: u64, p1: u32, control: u32) {
        let slot = self.dma.virt + (self.index * TRB_BYTES) as u64;
        // SAFETY: inside our own ring frame; the control dword (carrying the
        // cycle bit that hands ownership to the controller) is written last.
        unsafe {
            core::ptr::write_volatile(slot as *mut u64, p0);
            core::ptr::write_volatile((slot + 8) as *mut u32, p1);
            core::ptr::write_volatile((slot + 12) as *mut u32, control | self.cycle);
        }
        self.index += 1;
        if self.index == RING_LEN - 1 {
            // Hand the Link TRB over with the current cycle, then flip.
            let link = self.dma.virt + ((RING_LEN - 1) * TRB_BYTES) as u64;
            // SAFETY: as above.
            unsafe {
                core::ptr::write_volatile(
                    (link + 12) as *mut u32,
                    (TRB_LINK << 10) | TRB_TOGGLE_CYCLE | self.cycle,
                );
            }
            self.index = 0;
            self.cycle ^= 1;
        }
    }
}

/// The event ring the controller produces into.
struct EventRing {
    segment: Dma,
    _erst: Dma,
    index: usize,
    cycle: u32,
}

/// A completion read off the event ring.
#[derive(Debug, Clone, Copy)]
struct Event {
    kind: u32,
    completion_code: u8,
    slot_id: u8,
    parameter: u64,
    /// Bytes NOT transferred, as reported in the transfer event.
    residual: u32,
}

pub struct Xhci {
    op: u64,
    db: u64,
    rt: u64,
    cmd: Ring,
    events: EventRing,
    _dcbaa: Dma,
    _scratchpads: alloc::vec::Vec<Dma>,
    /// Input/device contexts and the control transfer ring for the one device
    /// this driver talks to.
    input_ctx: Dma,
    device_ctx: Dma,
    ep0: Ring,
    hid_ring: Ring,
    hid_buffer: Dma,
    context_size: usize,
    slot_id: u8,
    port: u8,
    hid_endpoint: Option<u8>,
    hid_packet: usize,
}

// Only ever reached through STATE's mutex.
unsafe impl Send for Xhci {}

impl Xhci {
    fn read32(base: u64, off: u64) -> u32 {
        // SAFETY: register inside the mapped BAR window.
        unsafe { core::ptr::read_volatile((base + off) as *const u32) }
    }

    fn write32(base: u64, off: u64, v: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) };
    }

    fn write64(base: u64, off: u64, v: u64) {
        // The specification requires 64-bit registers be written as two
        // dwords, low first, on 32-bit-capable implementations; doing it that
        // way is always correct and avoids depending on the controller's
        // tolerance for a single 64-bit store.
        Self::write32(base, off, v as u32);
        Self::write32(base, off + 4, (v >> 32) as u32);
    }

    fn portsc(&self, port: u8) -> u32 {
        Self::read32(
            self.op,
            OP_PORTSC_BASE + (port as u64 - 1) * OP_PORT_STRIDE,
        )
    }

    fn set_portsc(&self, port: u8, value: u32) {
        Self::write32(
            self.op,
            OP_PORTSC_BASE + (port as u64 - 1) * OP_PORT_STRIDE,
            value,
        );
    }

    /// Ring a doorbell. Slot 0 is the command ring; slot N endpoint E is a
    /// transfer ring.
    fn doorbell(&self, slot: u8, target: u32) {
        Self::write32(self.db, slot as u64 * 4, target);
    }

    /// Take the next event, if the controller has produced one.
    fn poll_event(&mut self) -> Option<Event> {
        let slot = self.events.segment.virt + (self.events.index * TRB_BYTES) as u64;
        // SAFETY: inside our own event-ring segment.
        let (parameter, status, control) = unsafe {
            (
                core::ptr::read_volatile(slot as *const u64),
                core::ptr::read_volatile((slot + 8) as *const u32),
                core::ptr::read_volatile((slot + 12) as *const u32),
            )
        };
        if control & TRB_CYCLE != self.events.cycle {
            return None;
        }
        let event = Event {
            kind: (control >> 10) & 0x3F,
            completion_code: (status >> 24) as u8,
            slot_id: (control >> 24) as u8,
            parameter,
            residual: status & 0x00FF_FFFF,
        };
        self.events.index += 1;
        if self.events.index == RING_LEN {
            self.events.index = 0;
            self.events.cycle ^= 1;
        }
        let deq = self.events.segment.phys + (self.events.index * TRB_BYTES) as u64;
        // Bit 3 is the Event Handler Busy flag, write-1-to-clear.
        Self::write64(self.rt, RT_IR0 + IR_ERDP, deq | (1 << 3));
        Some(event)
    }

    /// Wait for an event of `kind`, discarding port-status noise.
    fn await_event(&mut self, kind: u32, timeout_ms: u64) -> Option<Event> {
        let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
        loop {
            while let Some(event) = self.poll_event() {
                if event.kind == kind {
                    return Some(event);
                }
                // A port-status-change event during enumeration is expected
                // and carries no result; anything else is worth naming,
                // because a completion nobody asked for usually means the
                // driver's ring state and the controller's have diverged.
                if event.kind != TRB_PORT_STATUS_CHANGE {
                    serial_println!(
                        "[ITISYOU:USB] xhci unexpected_event kind={} code={}",
                        event.kind,
                        event.completion_code
                    );
                }
            }
            if !deadline.pending() {
                return None;
            }
        }
    }

    /// Post a command TRB and wait for its completion event.
    fn command(&mut self, p0: u64, p1: u32, control: u32, timeout_ms: u64) -> Option<Event> {
        self.cmd.push(p0, p1, control);
        self.doorbell(0, 0);
        self.await_event(TRB_COMMAND_COMPLETION, timeout_ms)
    }

    /// Byte offset of a context within a device or input context structure.
    /// Contexts are 32 or 64 bytes depending on HCCPARAMS1.CSZ.
    fn ctx_offset(&self, index: usize) -> u64 {
        (index * self.context_size) as u64
    }
}

pub struct XhciDriver;
pub static XHCI_DRIVER: XhciDriver = XhciDriver;

static STATE: Mutex<Option<Xhci>> = Mutex::new(None);

pub fn present() -> bool {
    STATE.lock().is_some()
}

pub fn with<R>(f: impl FnOnce(&mut Xhci) -> R) -> Option<R> {
    STATE.lock().as_mut().map(f)
}

impl Driver for XhciDriver {
    fn name(&self) -> &'static str {
        "xhci"
    }

    fn probe(&self, dev: &Device) -> bool {
        dev.id().is_usb() && dev.id().usb_kind() == kernel_core::pci::UsbKind::Xhci
    }

    fn attach(&self, dev: &Device) -> Result<(), DriverError> {
        dev.pci
            .enable_command_bits(super::pci::CMD_MEM_SPACE | super::pci::CMD_BUS_MASTER);
        let Some(Bar::Memory { addr, size, .. }) = dev.bars.first().copied() else {
            return Err(DriverError::InitFailed("xhci needs a memory BAR"));
        };
        let window = core::cmp::max(size, 0x2000);
        let base = paging::map_mmio(addr, window)
            .map_err(|_| DriverError::InitFailed("xhci BAR map failed"))?;

        let caplength = (Xhci::read32(base, CAP_CAPLENGTH) & 0xFF) as u64;
        let hcsparams1 = Xhci::read32(base, CAP_HCSPARAMS1);
        let hcsparams2 = Xhci::read32(base, CAP_HCSPARAMS2);
        let hccparams1 = Xhci::read32(base, CAP_HCCPARAMS1);
        let dboff = (Xhci::read32(base, CAP_DBOFF) & !0x3) as u64;
        let rtsoff = (Xhci::read32(base, CAP_RTSOFF) & !0x1F) as u64;
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
        let context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };
        let op = base + caplength;
        let db = base + dboff;
        let rt = base + rtsoff;

        // Halt, then reset. Resetting a running controller is undefined.
        let cmd = Xhci::read32(op, OP_USBCMD);
        Xhci::write32(op, OP_USBCMD, cmd & !USBCMD_RS);
        let mut deadline = crate::interrupts::Deadline::after_ms(100);
        while Xhci::read32(op, OP_USBSTS) & USBSTS_HCH == 0 && deadline.pending() {}
        Xhci::write32(op, OP_USBCMD, USBCMD_HCRST);
        let mut deadline = crate::interrupts::Deadline::after_ms(200);
        while (Xhci::read32(op, OP_USBCMD) & USBCMD_HCRST != 0
            || Xhci::read32(op, OP_USBSTS) & USBSTS_CNR != 0)
            && deadline.pending()
        {}
        if Xhci::read32(op, OP_USBSTS) & USBSTS_CNR != 0 {
            return Err(DriverError::InitFailed("xhci controller not ready"));
        }

        // Device context base address array, plus scratchpad buffers if the
        // controller asked for them. Skipping scratchpads on a controller that
        // requires them is a hang with no diagnostic, so they are allocated
        // whenever the count is non-zero.
        let dcbaa = Dma::alloc().ok_or(DriverError::InitFailed("xhci DCBAA"))?;
        let scratch_count =
            (((hcsparams2 >> 21) & 0x1F) << 5 | ((hcsparams2 >> 27) & 0x1F)) as usize;
        let mut scratchpads = alloc::vec::Vec::new();
        if scratch_count > 0 {
            let array = Dma::alloc().ok_or(DriverError::InitFailed("xhci scratchpad array"))?;
            for i in 0..scratch_count {
                let buf = Dma::alloc().ok_or(DriverError::InitFailed("xhci scratchpad"))?;
                // SAFETY: inside our own array frame.
                unsafe {
                    core::ptr::write_volatile((array.virt + (i * 8) as u64) as *mut u64, buf.phys)
                };
                scratchpads.push(buf);
            }
            // SAFETY: DCBAA slot 0 is the scratchpad array pointer.
            unsafe { core::ptr::write_volatile(dcbaa.virt as *mut u64, array.phys) };
            scratchpads.push(array);
        }

        let cmd_ring = Ring::new().ok_or(DriverError::InitFailed("xhci command ring"))?;
        let segment = Dma::alloc().ok_or(DriverError::InitFailed("xhci event segment"))?;
        let erst = Dma::alloc().ok_or(DriverError::InitFailed("xhci ERST"))?;
        // SAFETY: one ERST entry: segment base and size.
        unsafe {
            core::ptr::write_volatile(erst.virt as *mut u64, segment.phys);
            core::ptr::write_volatile((erst.virt + 8) as *mut u32, RING_LEN as u32);
            core::ptr::write_volatile((erst.virt + 12) as *mut u32, 0);
        }

        Xhci::write32(op, OP_CONFIG, max_slots as u32);
        Xhci::write64(op, OP_DCBAAP, dcbaa.phys);
        Xhci::write64(op, OP_CRCR, cmd_ring.dma.phys | 1);
        Xhci::write32(rt, RT_IR0 + IR_ERSTSZ, 1);
        Xhci::write64(rt, RT_IR0 + IR_ERDP, segment.phys | (1 << 3));
        Xhci::write64(rt, RT_IR0 + IR_ERSTBA, erst.phys);
        // Run. Interrupts stay masked: the event ring is polled.
        Xhci::write32(op, OP_USBCMD, USBCMD_RS);

        let mut xhci = Xhci {
            op,
            db,
            rt,
            cmd: cmd_ring,
            events: EventRing {
                segment,
                _erst: erst,
                index: 0,
                cycle: 1,
            },
            _dcbaa: dcbaa,
            _scratchpads: scratchpads,
            input_ctx: Dma::alloc().ok_or(DriverError::InitFailed("xhci input context"))?,
            device_ctx: Dma::alloc().ok_or(DriverError::InitFailed("xhci device context"))?,
            ep0: Ring::new().ok_or(DriverError::InitFailed("xhci ep0 ring"))?,
            hid_ring: Ring::new().ok_or(DriverError::InitFailed("xhci hid ring"))?,
            hid_buffer: Dma::alloc().ok_or(DriverError::InitFailed("xhci hid buffer"))?,
            context_size,
            slot_id: 0,
            port: 0,
            hid_endpoint: None,
            hid_packet: 8,
        };

        serial_println!(
            "[ITISYOU:USB] xhci ready caplength={caplength} slots={max_slots} ports={max_ports} \
context_size={context_size} scratchpads={scratch_count}"
        );

        match xhci.enumerate(max_ports) {
            Ok(()) => {}
            Err(e) => {
                serial_println!("[ITISYOU:USB] xhci enumerate_failed err={e}");
                *STATE.lock() = Some(xhci);
                return Ok(());
            }
        }
        *STATE.lock() = Some(xhci);
        Ok(())
    }
}

impl Xhci {
    /// Find a connected port, reset it, and bring the device to Addressed,
    /// then read its descriptors and configure the HID interrupt endpoint.
    fn enumerate(&mut self, max_ports: u8) -> Result<(), &'static str> {
        let port = (1..=max_ports)
            .find(|p| self.portsc(*p) & PORTSC_CCS != 0)
            .ok_or("no connected port")?;
        self.port = port;

        // USB2 ports need an explicit reset to become enabled; USB3 ports
        // enable themselves on connect. Checking PED first covers both without
        // having to know which protocol the port speaks.
        if self.portsc(port) & PORTSC_PED == 0 {
            let value = (self.portsc(port) & !PORTSC_RW1CS) | PORTSC_PR;
            self.set_portsc(port, value);
            let mut deadline = crate::interrupts::Deadline::after_ms(200);
            while self.portsc(port) & PORTSC_PED == 0 && deadline.pending() {}
        }
        let portsc = self.portsc(port);
        if portsc & PORTSC_PED == 0 {
            return Err("port never enabled");
        }
        let speed = (portsc >> 10) & 0x0F;

        let event = self
            .command(0, 0, TRB_ENABLE_SLOT << 10, 200)
            .ok_or("enable slot timed out")?;
        if event.completion_code != 1 {
            return Err("enable slot failed");
        }
        self.slot_id = event.slot_id;

        // Input context: control context enables A0 (slot) and A1 (EP0).
        let input = self.input_ctx.virt;
        // SAFETY: our own zeroed context frame; every offset below is inside
        // it (three contexts of at most 64 bytes in a 4 KiB frame).
        unsafe {
            core::ptr::write_volatile((input + 4) as *mut u32, 0b11);
            // Slot context is the second context in an INPUT context.
            let slot = input + self.ctx_offset(1);
            // Context entries = 1, speed, root hub port number.
            core::ptr::write_volatile(slot as *mut u32, (1 << 27) | (speed << 20));
            core::ptr::write_volatile((slot + 4) as *mut u32, (port as u32) << 16);
            // EP0 context: control endpoint, error count 3, max packet size.
            let ep0 = input + self.ctx_offset(2);
            let mps = max_packet_for(speed);
            core::ptr::write_volatile((ep0 + 4) as *mut u32, (4 << 3) | (3 << 1) | (mps << 16));
            core::ptr::write_volatile((ep0 + 8) as *mut u64, self.ep0.dma.phys | 1);
            self.hid_packet = mps as usize;
        }
        // Publish the device context before addressing: the controller writes
        // its output there.
        // SAFETY: DCBAA slot for this device.
        unsafe {
            core::ptr::write_volatile(
                (self._dcbaa.virt + self.slot_id as u64 * 8) as *mut u64,
                self.device_ctx.phys,
            )
        };

        let event = self
            .command(
                self.input_ctx.phys,
                0,
                (TRB_ADDRESS_DEVICE << 10) | ((self.slot_id as u32) << 24),
                500,
            )
            .ok_or("address device timed out")?;
        if event.completion_code != 1 {
            return Err("address device failed");
        }

        let mut descriptor = [0u8; 18];
        self.control_in(0x80, 6, 0x0100, 0, &mut descriptor)?;
        let device = kernel_core::usb::DeviceDescriptor::parse(&descriptor)
            .ok_or("bad device descriptor")?;
        serial_println!(
            "[ITISYOU:USB] xhci device vendor={:#06x} product={:#06x} class={:#04x} \
max_packet={}",
            device.vendor,
            device.product,
            device.device_class,
            device.max_packet_size,
        );

        // Configuration descriptor: first the 9-byte header to learn the total
        // length, then the whole thing.
        let mut header = [0u8; 9];
        self.control_in(0x80, 6, 0x0200, 0, &mut header)?;
        let total = u16::from_le_bytes([header[2], header[3]]) as usize;
        if total > 256 {
            return Err("configuration descriptor too large");
        }
        let mut config = [0u8; 256];
        self.control_in(0x80, 6, 0x0200, 0, &mut config[..total])?;
        let hid = kernel_core::usb::find_hid_interrupt_endpoint(&config[..total])
            .ok_or("no HID interrupt endpoint")?;
        serial_println!(
            "[ITISYOU:USB] xhci hid iface={} endpoint={:#04x} packet={} interval={}",
            hid.interface,
            hid.endpoint,
            hid.max_packet,
            hid.interval,
        );
        self.hid_endpoint = Some(hid.endpoint);
        Ok(())
    }

    /// A control IN transfer on EP0: Setup, Data, Status.
    fn control_in(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        out: &mut [u8],
    ) -> Result<(), &'static str> {
        let length = out.len() as u16;
        let setup = (request_type as u64)
            | ((request as u64) << 8)
            | ((value as u64) << 16)
            | ((index as u64) << 32)
            | ((length as u64) << 48);
        // Setup stage: the 8 setup bytes travel IN the TRB (immediate data).
        self.ep0.push(
            setup,
            8,
            (TRB_SETUP << 10) | TRB_IDT | (3 << 16), // TRT = IN data stage
        );
        self.ep0.push(
            self.hid_buffer.phys,
            length as u32,
            (TRB_DATA << 10) | (1 << 16) | TRB_ISP | TRB_ENT,
        );
        self.ep0
            .push(0, 0, (TRB_STATUS << 10) | TRB_IOC);
        self.doorbell(self.slot_id, 1);

        let event = self
            .await_event(TRB_TRANSFER_EVENT, 500)
            .ok_or("control transfer timed out")?;
        if event.completion_code != 1 && event.completion_code != 13 {
            return Err("control transfer failed");
        }
        let _ = event.parameter;
        // SAFETY: our own DMA buffer, at least `out.len()` bytes, and the
        // controller has finished with it (the transfer event arrived).
        unsafe {
            core::ptr::copy_nonoverlapping(self.hid_buffer.virt as *const u8, out.as_mut_ptr(), out.len())
        };
        Ok(())
    }

    /// Configure and poll the HID interrupt endpoint once.
    ///
    /// Returns the report bytes if one arrived within the deadline.
    pub fn poll_hid(&mut self, out: &mut [u8], timeout_ms: u64) -> Option<usize> {
        let endpoint = self.hid_endpoint?;
        let ep_num = endpoint & 0x0F;
        // Endpoint context index: 2 * ep + 1 for an IN endpoint.
        let dci = (ep_num * 2 + 1) as usize;
        if !self.configure_hid_endpoint(dci) {
            return None;
        }
        let len = core::cmp::min(out.len(), self.hid_packet);
        self.hid_ring.push(
            self.hid_buffer.phys,
            len as u32,
            (TRB_NORMAL << 10) | TRB_IOC | TRB_ISP,
        );
        self.doorbell(self.slot_id, dci as u32);
        let event = self.await_event(TRB_TRANSFER_EVENT, timeout_ms)?;
        if event.completion_code != 1 && event.completion_code != 13 {
            return None;
        }
        let received = len - core::cmp::min(event.residual as usize, len);
        // SAFETY: our own DMA buffer; the controller is finished with it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.hid_buffer.virt as *const u8,
                out.as_mut_ptr(),
                received,
            )
        };
        Some(received)
    }

    /// Add the interrupt endpoint to the device with a Configure Endpoint
    /// command. Idempotent: repeated calls re-issue the same configuration.
    fn configure_hid_endpoint(&mut self, dci: usize) -> bool {
        let input = self.input_ctx.virt;
        // SAFETY: our own context frame.
        unsafe {
            // Drop nothing; add the slot context and this endpoint.
            core::ptr::write_volatile(input as *mut u32, 0);
            core::ptr::write_volatile((input + 4) as *mut u32, 1 | (1 << dci));
            let slot = input + self.ctx_offset(1);
            let entries = core::ptr::read_volatile(slot as *const u32) & !(0x1F << 27);
            core::ptr::write_volatile(slot as *mut u32, entries | ((dci as u32) << 27));
            let ep = input + self.ctx_offset(1 + dci);
            core::ptr::write_bytes(ep as *mut u8, 0, self.context_size);
            // Interrupt IN endpoint, error count 3, max packet, interval 8.
            core::ptr::write_volatile(ep as *mut u32, 8 << 16);
            core::ptr::write_volatile(
                (ep + 4) as *mut u32,
                (7 << 3) | (3 << 1) | ((self.hid_packet as u32) << 16),
            );
            core::ptr::write_volatile((ep + 8) as *mut u64, self.hid_ring.dma.phys | 1);
            core::ptr::write_volatile((ep + 16) as *mut u32, self.hid_packet as u32);
        }
        let slot_id = self.slot_id;
        match self.command(
            self.input_ctx.phys,
            0,
            (TRB_CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
            500,
        ) {
            Some(e) if e.completion_code == 1 => true,
            other => {
                serial_println!(
                    "[ITISYOU:USB] xhci configure_endpoint failed code={}",
                    other.map(|e| e.completion_code).unwrap_or(0)
                );
                false
            }
        }
    }
}

/// Default control-endpoint packet size for a port speed.
fn max_packet_for(speed: u32) -> u32 {
    match speed {
        1 => 8,   // full speed (a full-speed device may report 8/16/32/64)
        2 => 8,   // low speed
        3 => 64,  // high speed
        _ => 512, // super speed and above
    }
}
