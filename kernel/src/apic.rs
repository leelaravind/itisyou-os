//! Local APIC, I/O APIC and MSI (V0.8).
//!
//! The V0.1–V0.7 interrupt path is the legacy 8259 PIC, and it is verified:
//! the timer, PS/2 keyboard and mouse all deliver through it. This module adds
//! the modern path **alongside** it rather than replacing it, because a
//! working interrupt path is not something to trade for a newer one without
//! evidence the newer one carries the same traffic.
//!
//! What that means concretely:
//!
//! * The **local APIC** is enabled and proved to deliver, using its own timer
//!   in one-shot mode on a dedicated vector. That is a real interrupt arriving
//!   through the APIC, counted by a handler, with nothing else depending on it.
//! * **MSI** is programmed on the NIC and proved to deliver: a message-signalled
//!   interrupt raised by real network traffic reaches a vector we chose. This
//!   is the interesting claim — MSI needs the local APIC to be enabled, the
//!   message address to encode the right destination, and bus mastering to
//!   work, so a delivered MSI exercises the whole chain.
//! * The **I/O APIC** is discovered, mapped, and a redirection entry is
//!   programmed and read back. Line-based IRQs stay on the PIC: rerouting the
//!   timer or the keyboard would put a verified V0.7 path at risk for no gain
//!   this milestone needs. `docs/KNOWN_LIMITATIONS.md` says so plainly rather
//!   than letting "IOAPIC support" imply more than was done.

use crate::memory::paging;
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::pci::Bar;
use spin::Mutex;
use x86_64::registers::model_specific::Msr;

/// `IA32_APIC_BASE`: bit 11 enables the local APIC, bits 12..51 hold its
/// physical base address.
const IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_ENABLE: u64 = 1 << 11;

// Local APIC register offsets.
const LAPIC_ID: u64 = 0x020;
const LAPIC_VERSION: u64 = 0x030;
const LAPIC_EOI: u64 = 0x0B0;
const LAPIC_SVR: u64 = 0x0F0;
const LAPIC_LVT_TIMER: u64 = 0x320;
const LAPIC_TIMER_INIT: u64 = 0x380;
const LAPIC_TIMER_CURRENT: u64 = 0x390;
const LAPIC_TIMER_DIVIDE: u64 = 0x3E0;

/// SVR bit 8 enables the APIC; the low byte is the spurious vector.
const SVR_ENABLE: u32 = 1 << 8;
/// LVT bit 16 masks the entry.
const LVT_MASKED: u32 = 1 << 16;

/// The I/O APIC's architectural address on every PC-class machine QEMU
/// emulates. Discovering it through ACPI would be the general answer; this
/// kernel already takes the RSDP from the bootloader and does not yet parse
/// the MADT, so the fixed address is used and the assumption is stated.
const IOAPIC_BASE_PHYS: u64 = 0xFEC0_0000;
const IOAPIC_REGSEL: u64 = 0x00;
const IOAPIC_WIN: u64 = 0x10;
const IOAPIC_REG_ID: u32 = 0x00;
const IOAPIC_REG_VERSION: u32 = 0x01;
const IOAPIC_REG_REDIR_BASE: u32 = 0x10;

/// MSI capability layout (PCI capability id 0x05).
const MSI_CTRL_ENABLE: u16 = 1 << 0;
const MSI_CTRL_64BIT: u16 = 1 << 7;

/// The message address every MSI on this platform targets: the local APIC's
/// architectural window, with the destination APIC id in bits 12..19.
const MSI_ADDRESS_BASE: u32 = 0xFEE0_0000;

struct Apic {
    lapic: u64,
    ioapic: Option<u64>,
    apic_id: u8,
}

static APIC: Mutex<Option<Apic>> = Mutex::new(None);

/// Interrupts delivered through the local APIC timer.
pub static TIMER_COUNT: AtomicU64 = AtomicU64::new(0);
/// Message-signalled interrupts delivered.
pub static MSI_COUNT: AtomicU64 = AtomicU64::new(0);
/// Spurious interrupts the APIC reported (should stay zero).
pub static SPURIOUS_COUNT: AtomicU64 = AtomicU64::new(0);

fn read_lapic(base: u64, offset: u64) -> u32 {
    // SAFETY: offset is a local-APIC register inside the mapped page.
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

fn write_lapic(base: u64, offset: u64, value: u32) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) };
}

/// Signal end-of-interrupt to the local APIC.
///
/// Every handler for an APIC-delivered vector must call this. Missing it does
/// not fail loudly — the CPU simply never delivers another interrupt at or
/// below that priority, so the symptom is "interrupts stopped", which is why
/// the counters below exist.
pub fn eoi() {
    if let Some(apic) = APIC.lock().as_ref() {
        write_lapic(apic.lapic, LAPIC_EOI, 0);
    }
}

pub fn is_enabled() -> bool {
    APIC.lock().is_some()
}

/// Bring up the local APIC and map the I/O APIC. Idempotent.
pub fn init() -> bool {
    if APIC.lock().is_some() {
        return true;
    }
    // CPUID leaf 1, EDX bit 9: the CPU has a local APIC at all.
    let features = core::arch::x86_64::__cpuid(1);
    if features.edx & (1 << 9) == 0 {
        serial_println!("[ITISYOU:IRQ] apic absent=true");
        return false;
    }

    let mut base_msr = Msr::new(IA32_APIC_BASE);
    // SAFETY: reading IA32_APIC_BASE is architectural on any CPU that
    // advertised an APIC above.
    let base_value = unsafe { base_msr.read() };
    let phys = base_value & 0xFFFF_F000;
    // SAFETY: setting the global-enable bit; the address field is preserved.
    unsafe { base_msr.write(base_value | APIC_BASE_ENABLE) };

    let Ok(lapic) = paging::map_mmio(phys, 0x1000) else {
        serial_println!("[ITISYOU:IRQ] apic map_failed phys={phys:#x}");
        return false;
    };

    // Enable, and route spurious interrupts to a vector with a handler. A
    // spurious interrupt is normal hardware behaviour, not an error; what
    // would be an error is having no handler for it.
    write_lapic(
        lapic,
        LAPIC_SVR,
        SVR_ENABLE | crate::interrupts::VECTOR_SPURIOUS as u32,
    );

    let apic_id = (read_lapic(lapic, LAPIC_ID) >> 24) as u8;
    let version = read_lapic(lapic, LAPIC_VERSION);

    let ioapic = paging::map_mmio(IOAPIC_BASE_PHYS, 0x1000).ok();
    if ioapic.is_none() {
        serial_println!("[ITISYOU:IRQ] ioapic map_failed");
    }

    *APIC.lock() = Some(Apic {
        lapic,
        ioapic,
        apic_id,
    });
    serial_println!(
        "[ITISYOU:IRQ] lapic_ready id={apic_id} version={:#x} max_lvt={} base={phys:#x}",
        version & 0xFF,
        (version >> 16) & 0xFF,
    );
    true
}

fn ioapic_read(base: u64, reg: u32) -> u32 {
    // SAFETY: the two-register window is the I/O APIC's only access method;
    // both writes are inside the mapped page.
    unsafe {
        core::ptr::write_volatile((base + IOAPIC_REGSEL) as *mut u32, reg);
        core::ptr::read_volatile((base + IOAPIC_WIN) as *const u32)
    }
}

fn ioapic_write(base: u64, reg: u32, value: u32) {
    // SAFETY: as above.
    unsafe {
        core::ptr::write_volatile((base + IOAPIC_REGSEL) as *mut u32, reg);
        core::ptr::write_volatile((base + IOAPIC_WIN) as *mut u32, value);
    }
}

/// Report the I/O APIC's identity and program one redirection entry, reading
/// it back to prove the register window really works.
///
/// The entry is left MASKED: the line it describes is still served by the PIC,
/// and two controllers delivering the same IRQ would be worse than one.
pub fn ioapic_probe(irq: u8, vector: u8) -> bool {
    let guard = APIC.lock();
    let Some(apic) = guard.as_ref() else {
        return false;
    };
    let Some(base) = apic.ioapic else {
        return false;
    };
    let id = (ioapic_read(base, IOAPIC_REG_ID) >> 24) & 0x0F;
    let version = ioapic_read(base, IOAPIC_REG_VERSION);
    let max_redirection = (version >> 16) & 0xFF;
    if irq as u32 > max_redirection {
        serial_println!("[ITISYOU:IRQ] ioapic irq={irq} out_of_range max={max_redirection}");
        return false;
    }
    let reg = IOAPIC_REG_REDIR_BASE + 2 * irq as u32;
    // Fixed delivery, physical destination, active high, edge triggered,
    // masked. The destination goes in the high dword's top byte.
    let low = vector as u32 | LVT_MASKED;
    let high = (apic.apic_id as u32) << 24;
    ioapic_write(base, reg + 1, high);
    ioapic_write(base, reg, low);
    let read_low = ioapic_read(base, reg);
    let read_high = ioapic_read(base, reg + 1);
    let ok = read_low == low && read_high == high;
    serial_println!(
        "[ITISYOU:IRQ] ioapic id={id} version={:#x} max_redirection={max_redirection} \
irq={irq} vector={vector:#x} readback={} masked=true",
        version & 0xFF,
        if ok { "ok" } else { "mismatch" },
    );
    ok
}

/// Fire one local-APIC timer interrupt on [`crate::interrupts::VECTOR_APIC_TIMER`]
/// and wait for the handler to count it.
///
/// One-shot rather than periodic on purpose: this proves delivery without
/// introducing a second periodic timer alongside the PIT the scheduler already
/// depends on.
pub fn timer_oneshot(ticks: u32, timeout_ms: u64) -> bool {
    let before = TIMER_COUNT.load(Ordering::SeqCst);
    {
        let guard = APIC.lock();
        let Some(apic) = guard.as_ref() else {
            return false;
        };
        // Divide by 16, one-shot, unmasked, on our vector.
        write_lapic(apic.lapic, LAPIC_TIMER_DIVIDE, 0x3);
        write_lapic(
            apic.lapic,
            LAPIC_LVT_TIMER,
            crate::interrupts::VECTOR_APIC_TIMER as u32,
        );
        write_lapic(apic.lapic, LAPIC_TIMER_INIT, ticks);
    }
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    while deadline.pending() {
        if TIMER_COUNT.load(Ordering::SeqCst) > before {
            // Disarm so nothing else fires later and confuses a later count.
            if let Some(apic) = APIC.lock().as_ref() {
                write_lapic(apic.lapic, LAPIC_TIMER_INIT, 0);
                write_lapic(apic.lapic, LAPIC_LVT_TIMER, LVT_MASKED);
            }
            return true;
        }
    }
    if let Some(apic) = APIC.lock().as_ref() {
        let remaining = read_lapic(apic.lapic, LAPIC_TIMER_CURRENT);
        write_lapic(apic.lapic, LAPIC_TIMER_INIT, 0);
        write_lapic(apic.lapic, LAPIC_LVT_TIMER, LVT_MASKED);
        serial_println!("[ITISYOU:IRQ] apic_timer_timeout remaining={remaining}");
    }
    false
}

/// Program a PCI device's MSI capability to deliver `vector` to this CPU.
///
/// Returns false when the device has no MSI capability — which is a fact about
/// the device, not a failure, so the caller decides what to report.
pub fn enable_msi(dev: &crate::device::Device, vector: u8) -> bool {
    let Some((_, offset)) = dev
        .caps
        .iter()
        .find(|(id, _)| *id == kernel_core::pci::CapabilityId::Msi)
        .copied()
    else {
        return false;
    };
    let apic_id = match APIC.lock().as_ref() {
        Some(a) => a.apic_id,
        None => return false,
    };

    let control = (dev.pci.read_config(offset) >> 16) as u16;
    let addr = MSI_ADDRESS_BASE | ((apic_id as u32) << 12);
    // Message data: fixed delivery mode, edge triggered, our vector.
    let data = vector as u32;

    dev.pci.write_config(offset + 4, addr);
    if control & MSI_CTRL_64BIT != 0 {
        // 64-bit capable: the upper address dword sits between the low address
        // and the data register, so the data register moves.
        dev.pci.write_config(offset + 8, 0);
        dev.pci.write_config(offset + 12, data);
    } else {
        dev.pci.write_config(offset + 8, data);
    }
    // Enable last, so no message can be raised before the address and data are
    // valid — an MSI written to address zero would be a memory write to
    // physical page 0, not an interrupt.
    let new_control = control | MSI_CTRL_ENABLE;
    let cap_dword = dev.pci.read_config(offset);
    dev.pci.write_config(
        offset,
        (cap_dword & 0x0000_FFFF) | ((new_control as u32) << 16),
    );

    serial_println!(
        "[ITISYOU:IRQ] msi_enabled dev={:02x}:{:02x}.{} vector={vector:#x} address={addr:#x} \
64bit={}",
        dev.pci.bus,
        dev.pci.slot,
        dev.pci.func,
        control & MSI_CTRL_64BIT != 0,
    );
    true
}

/// Program a PCI device's MSI-X capability to deliver `vector` to this CPU.
///
/// MSI-X keeps its message table in a BAR rather than in config space, so this
/// has to find the table's BAR and offset, map it, and write one 16-byte
/// entry. The extra indirection is also the reason MSI-X is worth having:
/// per-vector addresses and masks live in memory the driver owns, instead of
/// a handful of config-space registers shared by the whole function.
///
/// Returns false when the device has no MSI-X capability, which is a fact
/// about the device rather than a failure.
pub fn enable_msix(dev: &crate::device::Device, vector: u8) -> bool {
    let Some((_, offset)) = dev
        .caps
        .iter()
        .find(|(id, _)| *id == kernel_core::pci::CapabilityId::MsiX)
        .copied()
    else {
        return false;
    };
    let apic_id = match APIC.lock().as_ref() {
        Some(a) => a.apic_id,
        None => return false,
    };

    let control = (dev.pci.read_config(offset) >> 16) as u16;
    let table_size = (control & 0x7FF) + 1;
    let table = dev.pci.read_config(offset + 4);
    let bir = (table & 0x7) as usize;
    let table_offset = (table & !0x7) as u64;

    let Some(Bar::Memory { addr, .. }) = dev.bars.get(bir).copied() else {
        serial_println!("[ITISYOU:IRQ] msix bir={bir} is not a memory BAR");
        return false;
    };
    // Map the page containing the table. Mapping from the table's own page
    // boundary keeps the window small: the rest of the BAR belongs to the
    // driver, and this code has no business holding a second mapping of it.
    let page = (addr + table_offset) & !0xFFF;
    let within = ((addr + table_offset) & 0xFFF) as usize;
    let Ok(mapped) = paging::map_mmio(page, 0x1000) else {
        serial_println!("[ITISYOU:IRQ] msix table map_failed");
        return false;
    };
    let entry = mapped + within as u64;

    let msg_addr = MSI_ADDRESS_BASE | ((apic_id as u32) << 12);
    // SAFETY: `entry` is inside the mapped MSI-X table page; each field is a
    // 32-bit register the specification places at these offsets.
    unsafe {
        core::ptr::write_volatile(entry as *mut u32, msg_addr);
        core::ptr::write_volatile((entry + 4) as *mut u32, 0);
        core::ptr::write_volatile((entry + 8) as *mut u32, vector as u32);
        // Vector control bit 0 is the per-vector mask; clearing it last means
        // no message can be raised while the address was still incomplete.
        core::ptr::write_volatile((entry + 12) as *mut u32, 0);
    }

    // Enable the capability and clear the function-wide mask.
    let new_control = (control | (1 << 15)) & !(1 << 14);
    let cap_dword = dev.pci.read_config(offset);
    dev.pci.write_config(
        offset,
        (cap_dword & 0x0000_FFFF) | ((new_control as u32) << 16),
    );

    serial_println!(
        "[ITISYOU:IRQ] msix_enabled dev={:02x}:{:02x}.{} vector={vector:#x} address={msg_addr:#x} \
table_size={table_size} bir={bir} table_offset={table_offset:#x}",
        dev.pci.bus,
        dev.pci.slot,
        dev.pci.func,
    );
    true
}

/// Snapshot of the delivery counters, for the shell and the tests.
pub fn counters() -> (u64, u64, u64) {
    (
        TIMER_COUNT.load(Ordering::SeqCst),
        MSI_COUNT.load(Ordering::SeqCst),
        SPURIOUS_COUNT.load(Ordering::SeqCst),
    )
}
