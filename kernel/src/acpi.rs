//! ACPI discovery (V0.9).
//!
//! The bootloader hands over the RSDP's physical address; everything after
//! that is firmware data read through the kernel's physical-memory window and
//! parsed by the host-tested `kernel_core::acpi`. Two things come out of it:
//!
//! * the **MADT** — where the I/O APIC really is and which global system
//!   interrupt each ISA IRQ arrives on (QEMU's timer is IRQ0 → GSI 2), which
//!   is what makes the I/O APIC cutover possible without guessing;
//! * the **FADT** plus the DSDT's `\_S5_` sleep type — what a soft power-off
//!   has to write, and where.
//!
//! A missing or malformed table is reported and left unused; the kernel keeps
//! the path it already had (the architectural I/O APIC address, the QEMU exit
//! device) rather than acting on data it could not validate.

use crate::memory::paging;
use crate::serial_println;
use kernel_core::acpi::{self, Fadt, Madt, Rsdp, SleepType};
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::VirtAddr;

/// Largest table this kernel will map and checksum. QEMU's DSDT is ~8 KiB; a
/// firmware length field claiming megabytes is refused, not followed.
const MAX_TABLE: usize = 256 * 1024;
const MAX_ROOT_ENTRIES: usize = 32;

#[derive(Clone, Copy)]
pub struct AcpiInfo {
    pub revision: u8,
    pub madt: Option<Madt>,
    pub fadt: Option<Fadt>,
    pub s5: Option<SleepType>,
}

static INFO: Mutex<Option<AcpiInfo>> = Mutex::new(None);

/// Firmware bytes at physical `addr`, if every page of the range is mapped in
/// the physical-memory window. The window is permanent, so the slice is too.
fn phys_bytes(addr: u64, len: usize) -> Option<&'static [u8]> {
    if addr == 0 || len == 0 || len > MAX_TABLE {
        return None;
    }
    let end = addr.checked_add(len as u64)?;
    let mut page = addr & !0xFFF;
    while page < end {
        paging::translate(VirtAddr::new(paging::phys_offset() + page))?;
        page += 0x1000;
    }
    // SAFETY: every page of [addr, addr+len) was just confirmed mapped in the
    // kernel's physical-memory window, which is never unmapped; the bytes are
    // only read, and `kernel_core::acpi` bounds every access by `len`.
    Some(unsafe { core::slice::from_raw_parts(paging::phys_to_virt(addr), len) })
}

/// A whole system description table at `addr`, bounded by its own (checked)
/// length field.
fn table_at(addr: u64) -> Option<&'static [u8]> {
    let head = phys_bytes(addr, acpi::SDT_HEADER_LEN)?;
    let header = acpi::peek_header(head).ok()?;
    phys_bytes(addr, header.length as usize)
}

/// Discover and validate the tables. Called once at boot, before the
/// interrupt controller is set up.
pub fn init(rsdp_addr: Option<u64>) {
    let Some(addr) = rsdp_addr else {
        serial_println!("[ITISYOU:ACPI] rsdp=absent");
        return;
    };
    let Some(bytes) = phys_bytes(addr, acpi::RSDP_V2_LEN) else {
        serial_println!("[ITISYOU:ACPI] rsdp=unmapped addr={addr:#x}");
        return;
    };
    let rsdp = match Rsdp::parse(bytes) {
        Ok(r) => r,
        Err(e) => {
            serial_println!("[ITISYOU:ACPI] rsdp refused={e:?} addr={addr:#x}");
            return;
        }
    };
    let (root, xsdt) = if rsdp.xsdt_address != 0 {
        (rsdp.xsdt_address, true)
    } else {
        (rsdp.rsdt_address as u64, false)
    };
    let mut entries = [0u64; MAX_ROOT_ENTRIES];
    let count = match table_at(root).map(|t| acpi::root_entries(t, xsdt, &mut entries)) {
        Some(Ok(n)) => n,
        other => {
            serial_println!(
                "[ITISYOU:ACPI] root={} refused={:?}",
                if xsdt { "XSDT" } else { "RSDT" },
                other.map(|r| r.err())
            );
            return;
        }
    };

    let mut info = AcpiInfo {
        revision: rsdp.revision,
        madt: None,
        fadt: None,
        s5: None,
    };
    for &entry in &entries[..count] {
        let Some(table) = table_at(entry) else {
            continue;
        };
        match &table[..4] {
            b"APIC" => match Madt::parse(table) {
                Ok(m) => info.madt = Some(m),
                Err(e) => serial_println!("[ITISYOU:ACPI] madt refused={e:?}"),
            },
            b"FACP" => match Fadt::parse(table) {
                Ok(f) => {
                    info.fadt = Some(f);
                    match table_at(f.dsdt).map(acpi::find_s5) {
                        Some(Ok(s5)) => info.s5 = Some(s5),
                        other => serial_println!(
                            "[ITISYOU:ACPI] s5 unavailable={:?}",
                            other.map(|r| r.err())
                        ),
                    }
                }
                Err(e) => serial_println!("[ITISYOU:ACPI] fadt refused={e:?}"),
            },
            _ => {}
        }
    }

    serial_println!(
        "[ITISYOU:ACPI] rsdp_revision={} root={} tables={count} madt={} fadt={} s5={}",
        rsdp.revision,
        if xsdt { "XSDT" } else { "RSDT" },
        if info.madt.is_some() { "ok" } else { "absent" },
        if info.fadt.is_some() { "ok" } else { "absent" },
        if info.s5.is_some() { "ok" } else { "absent" },
    );
    if let Some(m) = &info.madt {
        let io = m.ioapics.iter().flatten().count();
        let ovr = m.overrides.iter().flatten().count();
        let timer = m.isa_route(0);
        serial_println!(
            "[ITISYOU:ACPI] madt lapic={:#x} cpus={} ioapics={io} overrides={ovr} pcat_compat={} irq0_gsi={}",
            m.local_apic_address,
            m.cpus,
            m.pcat_compat,
            timer.gsi,
        );
        for a in m.ioapics.iter().flatten() {
            serial_println!(
                "[ITISYOU:ACPI] ioapic id={} address={:#x} gsi_base={}",
                a.id,
                a.address,
                a.gsi_base
            );
        }
    }
    if let (Some(f), Some(s5)) = (&info.fadt, &info.s5) {
        serial_println!(
            "[ITISYOU:ACPI] fadt pm1a_cnt={:#x} pm1b_cnt={:#x} sci={} smi_cmd={:#x} s5_slp_typ={}/{}",
            f.pm1a_control,
            f.pm1b_control,
            f.sci_interrupt,
            f.smi_command,
            s5.a,
            s5.b,
        );
    }
    *INFO.lock() = Some(info);
}

pub fn info() -> Option<AcpiInfo> {
    *INFO.lock()
}

pub fn madt() -> Option<Madt> {
    info().and_then(|i| i.madt)
}

/// Why a soft power-off could not be requested.
#[derive(Debug, Clone, Copy)]
pub enum PowerOffError {
    NoTables,
    /// ACPI mode could not be enabled (SCI_EN never set).
    EnableFailed,
    /// The request was written but the machine is still running.
    StillRunning,
}

/// Enter S5 (soft off) through the FADT's PM1 control block(s).
///
/// Returns only on failure: success means the machine has powered off.
pub fn power_off() -> PowerOffError {
    let Some(AcpiInfo {
        fadt: Some(f),
        s5: Some(s5),
        ..
    }) = info()
    else {
        return PowerOffError::NoTables;
    };
    let Ok(pm1a) = u16::try_from(f.pm1a_control) else {
        return PowerOffError::NoTables;
    };
    if pm1a == 0 {
        return PowerOffError::NoTables;
    }
    let mut cnt_a: Port<u16> = Port::new(pm1a);
    // ACPI mode: SCI_EN (bit 0) set means firmware has handed the PM registers
    // to the OS. If not, ask for it through the SMI command port and wait.
    // SAFETY: PM1a_CNT and SMI_CMD are the I/O ports the validated FADT names;
    // reading PM1 control and writing the documented ACPI_ENABLE value have no
    // effect beyond the ACPI mode switch.
    let enabled = unsafe { cnt_a.read() } & 1 != 0;
    if !enabled && f.smi_command != 0 && f.acpi_enable != 0 {
        let mut smi: Port<u8> = Port::new(f.smi_command as u16);
        unsafe { smi.write(f.acpi_enable) };
        let mut deadline = crate::interrupts::Deadline::after_ms(300);
        while deadline.pending() && unsafe { cnt_a.read() } & 1 == 0 {}
        if unsafe { cnt_a.read() } & 1 == 0 {
            return PowerOffError::EnableFailed;
        }
    }
    serial_println!(
        "[ITISYOU:POWER] s5 request pm1a_cnt={pm1a:#x} slp_typ={} acpi_mode={}",
        s5.a,
        if enabled { "firmware" } else { "enabled" }
    );
    x86_64::instructions::interrupts::disable();
    // SAFETY: writing SLP_TYP|SLP_EN to the PM1 control block(s) the FADT
    // names is the ACPI-specified soft-off request; the machine stops here.
    unsafe {
        let current = cnt_a.read();
        cnt_a.write(acpi::pm1_sleep_value(current, s5.a));
        if let Ok(pm1b) = u16::try_from(f.pm1b_control) {
            if pm1b != 0 {
                let mut cnt_b: Port<u16> = Port::new(pm1b);
                let current_b = cnt_b.read();
                cnt_b.write(acpi::pm1_sleep_value(current_b, s5.b));
            }
        }
    }
    // Real hardware can take a moment; if it is still running after that, the
    // request did not take.
    let start = crate::interrupts::tsc();
    let budget = crate::interrupts::cycles_for_ms(500);
    while crate::interrupts::tsc().wrapping_sub(start) < budget {
        core::hint::spin_loop();
    }
    x86_64::instructions::interrupts::enable();
    PowerOffError::StillRunning
}
