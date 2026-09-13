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
//! * The **I/O APIC** (V0.9) now carries the line-based IRQs. Its address and
//!   the GSI each ISA IRQ arrives on come from the ACPI MADT (QEMU routes the
//!   PIT's IRQ0 to GSI 2), the timer, keyboard and mouse keep their vectors,
//!   and both 8259s plus the local APIC's LINT0 (the virtual-wire input the PIC
//!   used) are masked — so the legacy path cannot deliver even by accident.
//!   V0.8 only programmed and read back one masked entry; see ADR-0019.

use crate::memory::paging;
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::acpi::Madt;
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
const LAPIC_LVT_LINT0: u64 = 0x350;
const LAPIC_TIMER_INIT: u64 = 0x380;
const LAPIC_TIMER_CURRENT: u64 = 0x390;
const LAPIC_TIMER_DIVIDE: u64 = 0x3E0;

/// SVR bit 8 enables the APIC; the low byte is the spurious vector.
const SVR_ENABLE: u32 = 1 << 8;
/// LVT bit 16 masks the entry.
const LVT_MASKED: u32 = 1 << 16;

/// The I/O APIC's architectural address, used only when ACPI supplied no MADT
/// (V0.9 reads the real address from the MADT and says which one it used).
const IOAPIC_BASE_PHYS: u64 = 0xFEC0_0000;
/// Redirection-entry bits.
const REDIR_ACTIVE_LOW: u32 = 1 << 13;
const REDIR_LEVEL: u32 = 1 << 15;
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
    /// First GSI this I/O APIC serves, and how many redirection entries it has.
    gsi_base: u32,
    entries: u32,
    apic_id: u8,
}

static APIC: Mutex<Option<Apic>> = Mutex::new(None);

/// The local APIC's mapped base, published for the EOI path. Interrupt
/// handlers must never take the `APIC` lock: the code they interrupted may
/// hold it, and on one CPU that is a deadlock.
static LAPIC_BASE: AtomicU64 = AtomicU64::new(0);

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
    let base = LAPIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        write_lapic(base, LAPIC_EOI, 0);
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

    // Where the I/O APIC is: the MADT's word when ACPI gave one, the
    // architectural address otherwise — and the log says which.
    let madt_ioapic = crate::acpi::madt().and_then(|m| m.ioapics.iter().flatten().next().copied());
    let (ioapic_phys, gsi_base, source) = match madt_ioapic {
        Some(a) => (a.address as u64, a.gsi_base, "madt"),
        None => (IOAPIC_BASE_PHYS, 0, "architectural"),
    };
    let ioapic = paging::map_mmio(ioapic_phys, 0x1000).ok();
    let entries = match ioapic {
        Some(base) => ((ioapic_read(base, IOAPIC_REG_VERSION) >> 16) & 0xFF) + 1,
        None => {
            serial_println!("[ITISYOU:IRQ] ioapic map_failed phys={ioapic_phys:#x}");
            0
        }
    };
    if let Some(base) = ioapic {
        serial_println!(
            "[ITISYOU:IRQ] ioapic id={} base={ioapic_phys:#x} source={source} gsi_base={gsi_base} entries={entries}",
            (ioapic_read(base, IOAPIC_REG_ID) >> 24) & 0x0F,
        );
    }

    LAPIC_BASE.store(lapic, Ordering::SeqCst);
    *APIC.lock() = Some(Apic {
        lapic,
        ioapic,
        gsi_base,
        entries,
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

/// The vector a legacy ISA IRQ has always used (the PIC's remapped window), so
/// the cutover changes the controller, not the IDT.
fn isa_vector(irq: u8) -> u8 {
    if irq < 8 {
        crate::interrupts::PIC_1_OFFSET + irq
    } else {
        crate::interrupts::PIC_2_OFFSET + (irq - 8)
    }
}

/// Redirection-register index for a GSI on this I/O APIC, if it serves it.
fn redir_reg(apic: &Apic, gsi: u32) -> Option<u32> {
    let rel = gsi.checked_sub(apic.gsi_base)?;
    (rel < apic.entries).then_some(IOAPIC_REG_REDIR_BASE + 2 * rel)
}

/// Read one redirection entry (low, high) for a GSI.
pub fn ioapic_entry(gsi: u32) -> Option<(u32, u32)> {
    let guard = APIC.lock();
    let apic = guard.as_ref()?;
    let base = apic.ioapic?;
    let reg = redir_reg(apic, gsi)?;
    Some((ioapic_read(base, reg), ioapic_read(base, reg + 1)))
}

/// Move the timer (IRQ0), PS/2 keyboard (IRQ1) and PS/2 mouse (IRQ12) from
/// the 8259s to the I/O APIC, using the MADT's routes, then retire the PIC.
///
/// Each line keeps its vector, so the IDT does not change; each keeps the mask
/// state it had on the PIC, so a line nobody enabled does not start firing.
/// Everything happens with interrupts disabled and in this order — arm the new
/// routes, mask the PICs and LINT0, flip the EOI target — so no interrupt is
/// ever acknowledged on the wrong controller. Returns false, changing nothing,
/// when there is no MADT or no mapped I/O APIC to route through.
pub fn cutover_legacy_irqs() -> bool {
    let Some(madt) = crate::acpi::madt() else {
        serial_println!("[ITISYOU:IRQ] cutover skipped reason=no_madt legacy_lines=pic");
        return false;
    };
    // The tick rate must be the same on both sides of the cutover: a line
    // delivered twice (or not at all) would silently double (or stall) every
    // scheduler quantum and every deadline. Measured on the TSC, which does not
    // depend on either controller.
    let before = ticks_over_ms(100);
    let done = x86_64::instructions::interrupts::without_interrupts(|| cutover_locked(&madt));
    let after = ticks_over_ms(100);
    // One tick of slack either way for where the 100 ms window starts.
    let preserved = before > 0 && after.abs_diff(before) <= 1;
    serial_println!(
        "[ITISYOU:IRQ] timer_rate ticks_per_100ms before={before} after={after} controller={} rate_preserved={preserved}",
        if done { "ioapic" } else { "pic" }
    );
    done
}

/// Timer ticks counted over `ms` milliseconds of TSC time (interrupts on).
fn ticks_over_ms(ms: u64) -> u64 {
    let start = crate::interrupts::tsc();
    let budget = crate::interrupts::cycles_for_ms(ms);
    let t0 = crate::interrupts::ticks();
    while crate::interrupts::tsc().wrapping_sub(start) < budget {
        core::hint::spin_loop();
    }
    crate::interrupts::ticks() - t0
}

fn cutover_locked(madt: &Madt) -> bool {
    let (pic1, pic2) = crate::interrupts::pic_masks();
    let guard = APIC.lock();
    let Some(apic) = guard.as_ref() else {
        return false;
    };
    let Some(base) = apic.ioapic else {
        serial_println!("[ITISYOU:IRQ] cutover skipped reason=no_ioapic legacy_lines=pic");
        return false;
    };
    let mut ok = true;
    for irq in [0u8, 1, 12] {
        let route = madt.isa_route(irq);
        let Some(reg) = redir_reg(apic, route.gsi) else {
            serial_println!(
                "[ITISYOU:IRQ] ioapic_route isa={irq} gsi={} out_of_range",
                route.gsi
            );
            ok = false;
            continue;
        };
        let was_masked = if irq < 8 {
            pic1 & (1 << irq) != 0
        } else {
            pic2 & (1 << (irq - 8)) != 0
        };
        let vector = isa_vector(irq);
        let low = vector as u32
            | if route.active_low {
                REDIR_ACTIVE_LOW
            } else {
                0
            }
            | if route.level { REDIR_LEVEL } else { 0 }
            | if was_masked { LVT_MASKED } else { 0 };
        let high = (apic.apic_id as u32) << 24;
        ioapic_write(base, reg + 1, high);
        ioapic_write(base, reg, low);
        let readback = ioapic_read(base, reg) == low && ioapic_read(base, reg + 1) == high;
        ok &= readback;
        serial_println!(
            "[ITISYOU:IRQ] ioapic_route isa={irq} gsi={} vector={vector:#x} trigger={} polarity={} masked={was_masked} readback={}",
            route.gsi,
            if route.level { "level" } else { "edge" },
            if route.active_low { "low" } else { "high" },
            if readback { "ok" } else { "mismatch" },
        );
    }
    if !ok {
        // A route that did not read back is not trusted with the timer: every
        // entry written above is masked again and the PIC stays in charge.
        for irq in [0u8, 1, 12] {
            if let Some(reg) = redir_reg(apic, madt.isa_route(irq).gsi) {
                ioapic_write(base, reg, LVT_MASKED);
            }
        }
        serial_println!("[ITISYOU:IRQ] cutover aborted legacy_lines=pic");
        return false;
    }
    // Retire the legacy path: both 8259s fully masked, and the local APIC's
    // LINT0 (the virtual-wire input they reached the CPU through) masked too.
    crate::interrupts::retire_pic();
    let lint0 = read_lapic(apic.lapic, LAPIC_LVT_LINT0);
    write_lapic(apic.lapic, LAPIC_LVT_LINT0, lint0 | LVT_MASKED);
    drop(guard);
    let (m1, m2) = crate::interrupts::pic_masks();
    serial_println!(
        "[ITISYOU:IRQ] pic_retired masks={m1:#04x}/{m2:#04x} lint0=masked legacy_lines=ioapic"
    );
    true
}

/// Mask or unmask the I/O APIC line carrying ISA `irq` (after the cutover).
pub fn set_isa_line_masked(irq: u8, masked: bool) {
    let Some(madt) = crate::acpi::madt() else {
        return;
    };
    x86_64::instructions::interrupts::without_interrupts(|| {
        let guard = APIC.lock();
        let Some(apic) = guard.as_ref() else {
            return;
        };
        let (Some(base), Some(reg)) = (apic.ioapic, redir_reg(apic, madt.isa_route(irq).gsi))
        else {
            return;
        };
        let low = ioapic_read(base, reg);
        ioapic_write(
            base,
            reg,
            if masked {
                low | LVT_MASKED
            } else {
                low & !LVT_MASKED
            },
        );
    });
}

/// Prove the timer really arrives through the I/O APIC: with its redirection
/// entry masked the tick count must stand still, and it must move again once
/// unmasked. Returns (ticks while masked, ticks after unmasking) over `ms`
/// milliseconds each, measured on the TSC (which does not depend on the timer).
pub fn prove_timer_route(ms: u64) -> Option<(u64, u64)> {
    if !crate::interrupts::legacy_via_ioapic() {
        return None;
    }
    let wait = |ms: u64| {
        let start = crate::interrupts::tsc();
        let budget = crate::interrupts::cycles_for_ms(ms);
        while crate::interrupts::tsc().wrapping_sub(start) < budget {
            core::hint::spin_loop();
        }
    };
    set_isa_line_masked(0, true);
    // Let an interrupt that was already in flight when the mask landed be
    // counted before the measurement starts, so it cannot be mistaken for a
    // delivery that bypassed the mask.
    wait(20);
    let before = crate::interrupts::ticks();
    wait(ms);
    let masked_ticks = crate::interrupts::ticks() - before;
    set_isa_line_masked(0, false);
    let resumed = crate::interrupts::ticks();
    wait(ms);
    let unmasked_ticks = crate::interrupts::ticks() - resumed;
    Some((masked_ticks, unmasked_ticks))
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
