//! UHCI (Universal Host Controller Interface) USB 1.1 driver (V0.6).
//!
//! The smallest correct USB host stack that reaches a *real* class driver:
//! reset the controller, reset the root port a device is on, enumerate the
//! device over control transfers (GET_DESCRIPTOR → SET_ADDRESS → configuration
//! → SET_PROTOCOL(boot)), then read HID boot reports off the interrupt-IN
//! endpoint. Chosen over xHCI as the smallest controller that QEMU emulates
//! fully with `usb-kbd`/`usb-mouse`, so HID input can actually be *verified*;
//! xHCI is the recommended follow-up. Descriptor + HID decoding is the
//! host-tested `kernel_core::usb`.
//!
//! Transfers are **polled** — no controller IRQ is wired — so the verified PIC
//! timer/PS-2 interrupt path is untouched (V0.6 keeps drivers interrupt-free).

use crate::device::{pci, Device, Driver, DriverError};
use crate::memory::{self, paging};
use crate::serial_println;
use crate::sync::Mutex;
use kernel_core::usb::{self, DeviceDescriptor};
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PhysFrame;

// UHCI I/O registers (relative to the I/O BAR base).
const USBCMD: u16 = 0x00;
const USBSTS: u16 = 0x02;
const USBINTR: u16 = 0x04;
const FRNUM: u16 = 0x06;
const FRBASEADD: u16 = 0x08;
const SOFMOD: u16 = 0x0C;
const PORTSC1: u16 = 0x10;

// USBCMD bits.
const CMD_RS: u16 = 1 << 0; // run/stop
const CMD_HCRESET: u16 = 1 << 1;
const CMD_GRESET: u16 = 1 << 2;
const CMD_MAXP: u16 = 1 << 7; // 64-byte max packet

// PORTSC bits.
const PORT_CCS: u16 = 1 << 0; // current connect status
const PORT_CSC: u16 = 1 << 1; // connect status change (write 1 to clear)
const PORT_PE: u16 = 1 << 2; // port enable
const PORT_PEC: u16 = 1 << 3; // port enable change (write 1 to clear)
const PORT_LS: u16 = 1 << 8; // low-speed device attached
const PORT_PR: u16 = 1 << 9; // port reset

// TD control/status bits (dword1).
const TD_ACTIVE: u32 = 1 << 23;
const TD_LS: u32 = 1 << 26; // low-speed
const TD_CERR3: u32 = 3 << 27; // 3 error retries
const TD_STATUS_MASK: u32 = 0x00FF_0000;

// TD token PIDs (dword2, bits 0-7).
const PID_SETUP: u32 = 0x2D;
const PID_IN: u32 = 0x69;
const PID_OUT: u32 = 0xE1;

// Link pointer bits.
const LP_TERMINATE: u32 = 1 << 0;
const LP_QH: u32 = 1 << 1;

// Work-frame layout (offsets within one 4 KiB DMA frame).
const OFF_QH: u64 = 0x000;
const OFF_TD0: u64 = 0x040; // TDs at 0x40, 0x60, 0x80, ... (32 B each)
const OFF_SETUP: u64 = 0x200;
const OFF_DATA: u64 = 0x400;

pub struct UhciDriver;
pub static UHCI_DRIVER: UhciDriver = UhciDriver;

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
        // SAFETY: fresh exclusively-owned frame; zero it before DMA use.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, 4096) };
        Some(DmaFrame {
            phys,
            virt,
            _frame: frame,
        })
    }
}

struct Uhci {
    io: u16,
    /// Held so the frame-list DMA the controller reads stays mapped.
    _frame_list: DmaFrame,
    work: DmaFrame,
    low_speed: bool,
    address: u8,
    /// HID interrupt-IN endpoint number + max packet size, once configured.
    hid_ep: Option<(u8, u16)>,
    hid_toggle: u32,
    kind: &'static str,
}

static STATE: Mutex<Option<Uhci>> = Mutex::new(None);

impl Driver for UhciDriver {
    fn name(&self) -> &'static str {
        "uhci"
    }

    fn probe(&self, dev: &Device) -> bool {
        dev.id().is_usb() && dev.id().usb_kind() == kernel_core::pci::UsbKind::Uhci
    }

    fn attach(&self, dev: &Device) -> Result<(), DriverError> {
        dev.pci
            .enable_command_bits(pci::CMD_IO_SPACE | pci::CMD_BUS_MASTER);
        // UHCI I/O registers live in the (only) I/O BAR (BAR4 on PIIX3).
        let io = dev
            .first_io_bar()
            .map(|(p, _)| p)
            .ok_or(DriverError::InitFailed("uhci needs an I/O BAR"))?;

        let frame_list = DmaFrame::alloc().ok_or(DriverError::InitFailed("uhci frame list"))?;
        let work = DmaFrame::alloc().ok_or(DriverError::InitFailed("uhci work frame"))?;

        // Global reset the controller, then host-controller reset.
        unsafe {
            wr16(io, USBCMD, CMD_GRESET);
            // UHCI 2.1.1: global reset must be asserted for at least 10 ms.
            crate::interrupts::delay_ms(20);
            wr16(io, USBCMD, 0);
            wr16(io, USBCMD, CMD_HCRESET);
            let mut deadline = crate::interrupts::Deadline::after_ms(100);
            while rd16(io, USBCMD) & CMD_HCRESET != 0 && deadline.pending() {}
            // Disable all interrupts (we poll), clear status.
            wr16(io, USBINTR, 0);
            wr16(io, USBSTS, 0xFFFF);
            wr8(io, SOFMOD, 64);
            // Every frame-list entry points at our QH (Q bit) so a one-shot
            // transfer runs whatever frame the controller is currently on.
            let qh_phys = (work.phys + OFF_QH) as u32;
            for i in 0..1024u64 {
                wr32_dma(frame_list.virt + i * 4, qh_phys | LP_QH);
            }
            wr32(io, FRBASEADD, frame_list.phys as u32);
            wr16(io, FRNUM, 0);
            // Run, 64-byte max packet, configure flag.
            wr16(io, USBCMD, CMD_RS | CMD_MAXP | (1 << 6));
        }

        let mut uhci = Uhci {
            io,
            _frame_list: frame_list,
            work,
            low_speed: false,
            address: 0,
            hid_ep: None,
            hid_toggle: 0,
            kind: "usb",
        };

        // Reset the first populated root port and enumerate its device.
        match uhci.reset_port() {
            Ok(low_speed) => uhci.low_speed = low_speed,
            Err(e) => {
                serial_println!("[ITISYOU:INFO] uhci no_device err={e}");
                // Still store the controller (initialized) even with no device.
                *STATE.lock() = Some(uhci);
                return Ok(());
            }
        }

        if let Err(e) = uhci.enumerate() {
            serial_println!("[ITISYOU:INFO] uhci enumerate_failed err={e}");
            *STATE.lock() = Some(uhci);
            return Err(DriverError::InitFailed(e));
        }

        serial_println!(
            "[ITISYOU:INFO] uhci ready io={:#06x} low_speed={} kind={}",
            uhci.io,
            uhci.low_speed,
            uhci.kind,
        );
        *STATE.lock() = Some(uhci);
        Ok(())
    }
}

impl Uhci {
    /// Reset the first root port reporting a connected device; returns whether
    /// the attached device is low-speed. Errors if no device is present.
    fn reset_port(&mut self) -> Result<bool, &'static str> {
        let io = self.io;
        // Only port 1 is used by our topology, but check both.
        for port in [PORTSC1, PORTSC1 + 2] {
            let sc = unsafe { rd16(io, port) };
            if sc & PORT_CCS == 0 {
                continue;
            }
            let low_speed = sc & PORT_LS != 0;
            unsafe {
                // Assert reset for ~50 ms, then clear and enable.
                wr16(io, port, PORT_PR);
                // USB 2.0 7.1.7.5: drive reset for at least 10 ms, then allow
                // the device its 10 ms recovery time before addressing it.
                crate::interrupts::delay_ms(50);
                wr16(io, port, sc & !PORT_PR);
                crate::interrupts::delay_ms(10);
                // Enable the port; clear connect/enable change (write-1-clear).
                wr16(io, port, PORT_PE | PORT_CSC | PORT_PEC);
                let mut deadline = crate::interrupts::Deadline::after_ms(100);
                while rd16(io, port) & PORT_PE == 0 && deadline.pending() {}
            }
            serial_println!(
                "[ITISYOU:INFO] uhci port_reset port={:#x} low_speed={low_speed}",
                port
            );
            return Ok(low_speed);
        }
        Err("no connected device")
    }

    /// Full enumeration: device descriptor → address → configuration →
    /// SET_PROTOCOL(boot), recording the HID interrupt-IN endpoint.
    fn enumerate(&mut self) -> Result<(), &'static str> {
        // Address 0, GET_DESCRIPTOR(device), first 18 bytes.
        let mut dev_desc = [0u8; 18];
        self.control_in(0, 0x80, 0x06, 0x0100, 0, &mut dev_desc)?;
        let desc = DeviceDescriptor::parse(&dev_desc).ok_or("bad device descriptor")?;
        serial_println!(
            "[ITISYOU:INFO] usb device vendor={:#06x} product={:#06x} usb={:#06x} class={:#04x} mps0={} configs={}",
            desc.vendor,
            desc.product,
            desc.usb_version,
            desc.class,
            desc.max_packet_size0,
            desc.num_configurations,
        );

        // SET_ADDRESS(1).
        self.control_out(0, 0x00, 0x05, 1, 0, &[])?;
        self.address = 1;
        // USB 2.0 9.2.6.3: a device has 2 ms to commit a new address.
        crate::interrupts::delay_ms(10);

        // GET_DESCRIPTOR(configuration): first 9 bytes for wTotalLength.
        let mut cfg_head = [0u8; 9];
        self.control_in(1, 0x80, 0x06, 0x0200, 0, &mut cfg_head)?;
        let total = u16::from_le_bytes([cfg_head[7], cfg_head[8]]).min(255) as usize;
        let mut cfg = [0u8; 255];
        let n = total.max(9).min(cfg.len());
        self.control_in(1, 0x80, 0x06, 0x0200, 0, &mut cfg[..n])?;

        let config_value = cfg[5];
        // SET_CONFIGURATION.
        self.control_out(1, 0x00, 0x09, config_value as u16, 0, &[])?;

        if let Some((iface, ep)) = usb::find_hid_interrupt_in(&cfg[..n]) {
            self.kind = if iface.protocol == usb::HID_PROTOCOL_MOUSE {
                "mouse"
            } else if iface.protocol == usb::HID_PROTOCOL_KEYBOARD {
                "keyboard"
            } else {
                "hid"
            };
            // SET_PROTOCOL(boot=0) on the HID interface (class request).
            let _ = self.control_out(1, 0x21, 0x0B, 0x0000, iface.interface_number as u16, &[]);
            self.hid_ep = Some((ep.number(), ep.max_packet_size));
            serial_println!(
                "[ITISYOU:INFO] usb hid iface={} proto={} ep={} mps={} kind={}",
                iface.interface_number,
                iface.protocol,
                ep.number(),
                ep.max_packet_size,
                self.kind,
            );
        } else {
            serial_println!("[ITISYOU:INFO] usb hid none");
        }
        Ok(())
    }

    /// A control IN transfer (SETUP → DATA IN → STATUS OUT). Fills `data`.
    fn control_in(
        &self,
        addr: u8,
        req_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
    ) -> Result<(), &'static str> {
        self.setup_packet(req_type, request, value, index, data.len() as u16);
        let tds = self.build_control(addr, PID_IN, data.len());
        self.run_and_wait(tds)?;
        // Copy the received bytes out of the data buffer.
        for (i, b) in data.iter_mut().enumerate() {
            *b = unsafe { rd8_dma(self.work.virt + OFF_DATA + i as u64) };
        }
        Ok(())
    }

    /// A control OUT transfer (SETUP → [DATA OUT] → STATUS IN).
    fn control_out(
        &self,
        addr: u8,
        req_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<(), &'static str> {
        self.setup_packet(req_type, request, value, index, data.len() as u16);
        for (i, b) in data.iter().enumerate() {
            unsafe { wr8_dma(self.work.virt + OFF_DATA + i as u64, *b) };
        }
        let tds = self.build_control(addr, PID_OUT, data.len());
        self.run_and_wait(tds)
    }

    fn setup_packet(&self, req_type: u8, request: u8, value: u16, index: u16, length: u16) {
        let p = self.work.virt + OFF_SETUP;
        unsafe {
            wr8_dma(p, req_type);
            wr8_dma(p + 1, request);
            wr8_dma(p + 2, value as u8);
            wr8_dma(p + 3, (value >> 8) as u8);
            wr8_dma(p + 4, index as u8);
            wr8_dma(p + 5, (index >> 8) as u8);
            wr8_dma(p + 6, length as u8);
            wr8_dma(p + 7, (length >> 8) as u8);
        }
    }

    /// Build the TD chain for a control transfer. `data_pid` is IN or OUT for
    /// the data stage; the status stage is the opposite direction. Returns the
    /// physical address of the first TD.
    fn build_control(&self, addr: u8, data_pid: u32, data_len: usize) -> u32 {
        let ls = if self.low_speed { TD_LS } else { 0 };
        let mut idx = 0u64;
        let td_phys = |i: u64| (self.work.phys + OFF_TD0 + i * 32) as u32;

        // SETUP TD: PID SETUP, 8 bytes, toggle 0.
        let setup_td = td_phys(idx);
        self.write_td(
            idx,
            td_phys(idx + 1),
            ls,
            PID_SETUP,
            addr,
            0,
            0,
            8,
            self.work.phys + OFF_SETUP,
        );
        idx += 1;

        // DATA stage: one TD (data fits in <=64 bytes for our uses), toggle 1.
        if data_len > 0 {
            self.write_td(
                idx,
                td_phys(idx + 1),
                ls,
                data_pid,
                addr,
                0,
                1,
                data_len as u16,
                self.work.phys + OFF_DATA,
            );
            idx += 1;
        }

        // STATUS TD: opposite direction, zero length, toggle 1, IOC.
        let status_pid = if data_pid == PID_IN { PID_OUT } else { PID_IN };
        self.write_td(idx, LP_TERMINATE, ls, status_pid, addr, 0, 1, 0, 0);

        // QH element → first TD.
        unsafe {
            wr32_dma(self.work.virt + OFF_QH, LP_TERMINATE); // head link: terminate
            wr32_dma(self.work.virt + OFF_QH + 4, setup_td); // element → SETUP
        }
        setup_td
    }

    /// Write one 32-byte TD at work-frame slot `i`.
    #[allow(clippy::too_many_arguments)]
    fn write_td(
        &self,
        i: u64,
        link: u32,
        ls: u32,
        pid: u32,
        addr: u8,
        endpoint: u8,
        toggle: u32,
        len: u16,
        buffer_phys: u64,
    ) {
        let base = self.work.virt + OFF_TD0 + i * 32;
        // maxlen field: (len-1) & 0x7FF, or 0x7FF for zero-length.
        let maxlen = if len == 0 {
            0x7FF
        } else {
            (len as u32 - 1) & 0x7FF
        };
        let token = pid
            | ((addr as u32 & 0x7F) << 8)
            | ((endpoint as u32 & 0x0F) << 15)
            | (toggle << 19)
            | (maxlen << 21);
        unsafe {
            wr32_dma(base, link);
            wr32_dma(base + 4, TD_ACTIVE | TD_CERR3 | ls);
            wr32_dma(base + 8, token);
            wr32_dma(base + 12, buffer_phys as u32);
        }
    }

    /// Point the QH at `first_td`, wait for the chain to finish (all TDs
    /// inactive) or time out, and check for errors.
    fn run_and_wait(&self, first_td: u32) -> Result<(), &'static str> {
        unsafe { wr32_dma(self.work.virt + OFF_QH + 4, first_td) };
        // The frame list already points every frame at the QH; poll the last
        // (STATUS) TD — walk the chain until an active/error TD is found.
        // Real-time bound: a control transfer through QEMU's UHCI model
        // completes in well under a millisecond when the host is idle, but an
        // iteration count collapses to microseconds of guest time under load.
        let mut deadline = crate::interrupts::Deadline::after_ms(1000);
        loop {
            let mut all_done = true;
            let mut err = false;
            // Up to 3 TDs (setup, data, status).
            for i in 0..3u64 {
                let cs = unsafe { rd32_dma(self.work.virt + OFF_TD0 + i * 32 + 4) };
                if cs & TD_ACTIVE != 0 {
                    all_done = false;
                    break;
                }
                // Error bits (excluding the harmless NAK-less states): stalled,
                // buffer error, babble, timeout/CRC.
                if cs & 0x007E_0000 != 0 {
                    err = true;
                }
                // The link's terminate bit marks the last TD in the chain.
                let link = unsafe { rd32_dma(self.work.virt + OFF_TD0 + i * 32) };
                if link & LP_TERMINATE != 0 {
                    break;
                }
            }
            if all_done {
                if err {
                    let st = unsafe { rd32_dma(self.work.virt + OFF_TD0 + 4) } & TD_STATUS_MASK;
                    let _ = st;
                    return Err("transfer error");
                }
                return Ok(());
            }
            if !deadline.pending() {
                break;
            }
        }
        Err("transfer timeout")
    }

    /// Read one HID boot report off the interrupt-IN endpoint. Returns the
    /// bytes actually received (may be empty if the device NAKs — no input).
    fn poll_hid(&mut self, out: &mut [u8]) -> usize {
        let Some((ep, mps)) = self.hid_ep else {
            return 0;
        };
        let ls = if self.low_speed { TD_LS } else { 0 };
        let want = (mps as usize).min(out.len());
        // Single IN TD on the interrupt endpoint, current toggle, no retries
        // (NAK just means "no report waiting" — don't spin on it).
        let base = self.work.virt + OFF_TD0;
        let maxlen = if want == 0 {
            0x7FF
        } else {
            (want as u32 - 1) & 0x7FF
        };
        let token = PID_IN
            | ((self.address as u32 & 0x7F) << 8)
            | ((ep as u32 & 0x0F) << 15)
            | (self.hid_toggle << 19)
            | (maxlen << 21);
        unsafe {
            wr32_dma(base, LP_TERMINATE);
            wr32_dma(base + 4, TD_ACTIVE | ls); // C_ERR=0: don't retry NAKs
            wr32_dma(base + 8, token);
            wr32_dma(base + 12, (self.work.phys + OFF_DATA) as u32);
            wr32_dma(
                self.work.virt + OFF_QH + 4,
                (self.work.phys + OFF_TD0) as u32,
            );
        }
        // Poll briefly; a boot report either arrives or the TD NAKs/inactivates.
        // Bounded in real time so a loaded host cannot silently shorten the
        // window to nothing (and so a wedged controller cannot stall the
        // desktop loop).
        let mut deadline = crate::interrupts::Deadline::after_ms(20);
        loop {
            let cs = unsafe { rd32_dma(base + 4) };
            if cs & TD_ACTIVE == 0 {
                // ActLen field (bits 0-10): received length is actlen+1, or 0.
                let actlen = cs & 0x7FF;
                if cs & 0x007E_0000 != 0 {
                    return 0; // error → treat as no data
                }
                let got = if actlen == 0x7FF {
                    0
                } else {
                    (actlen as usize + 1).min(want)
                };
                if got > 0 {
                    self.hid_toggle ^= 1;
                    for (i, b) in out.iter_mut().enumerate().take(got) {
                        *b = unsafe { rd8_dma(self.work.virt + OFF_DATA + i as u64) };
                    }
                }
                return got;
            }
            if !deadline.pending() {
                break;
            }
        }
        // Still active → cancel by clearing active; no data this poll.
        unsafe { wr32_dma(base + 4, 0) };
        0
    }
}

/// True once a UHCI controller is initialized.
pub fn available() -> bool {
    STATE.lock().is_some()
}

/// The enumerated device kind ("keyboard"/"mouse"/"usb"), for diagnostics.
pub fn device_kind() -> Option<&'static str> {
    STATE.lock().as_ref().map(|u| u.kind)
}

/// Poll the HID interrupt endpoint once; returns the report bytes received.
pub fn poll_hid_report(out: &mut [u8]) -> usize {
    match STATE.lock().as_mut() {
        Some(u) => u.poll_hid(out),
        None => 0,
    }
}

/// Does the enumerated device expose a HID interrupt endpoint?
pub fn has_hid() -> bool {
    STATE
        .lock()
        .as_ref()
        .map(|u| u.hid_ep.is_some())
        .unwrap_or(false)
}

// --- port I/O + DMA memory helpers ---

unsafe fn wr8(io: u16, off: u16, v: u8) {
    Port::<u8>::new(io + off).write(v)
}
unsafe fn wr16(io: u16, off: u16, v: u16) {
    Port::<u16>::new(io + off).write(v)
}
unsafe fn rd16(io: u16, off: u16) -> u16 {
    Port::<u16>::new(io + off).read()
}
unsafe fn wr32(io: u16, off: u16, v: u32) {
    Port::<u32>::new(io + off).write(v)
}

unsafe fn wr8_dma(virt: u64, v: u8) {
    (virt as *mut u8).write_volatile(v)
}
unsafe fn rd8_dma(virt: u64) -> u8 {
    (virt as *const u8).read_volatile()
}
unsafe fn wr32_dma(virt: u64, v: u32) {
    (virt as *mut u32).write_volatile(v)
}
unsafe fn rd32_dma(virt: u64) -> u32 {
    (virt as *const u32).read_volatile()
}
