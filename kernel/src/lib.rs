//! ITISYOU OS kernel library.
//!
//! The boot binaries (`itisyou-kernel`, `itisyou-kernel-selftest`,
//! `itisyou-kernel-panictest`) are thin wrappers over this library. All
//! subsystem logic lives here; pure algorithmic logic that also runs on the
//! host lives in `kernel-core`.

#![no_std]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod acpi;
pub mod apic;
pub mod audit;
pub mod bootstage;
pub mod capability;
pub mod cpu;
pub mod desktop;
pub mod device;
pub mod fs;
pub mod fs_disk;
pub mod gdt;
pub mod gfx;
pub mod harden;
pub mod input;
pub mod interrupts;
pub mod ipc;
pub mod memory;
pub mod net;
pub mod platform;
pub mod proc;
pub mod qemu;
pub mod selftest;
pub mod serial;
pub mod services;
pub mod shell;
pub mod syscall;
pub mod task;
pub mod user;

use bootloader_api::config::Mapping;
use bootloader_api::{BootInfo, BootloaderConfig};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use kernel_core::stage::Stage;

/// Bootloader handoff configuration shared by all boot binaries.
///
/// The full physical address space is mapped at a dynamic offset so the
/// physical memory manager and paging layer can reach every frame.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.kernel_stack_size = 128 * 1024;
    // The bootloader provides a linear framebuffer by default (firmware-
    // chosen mode); the kernel queries its real dimensions at runtime and
    // never hardcodes them (V0.5).
    config
};

/// When set, a panic exits QEMU with a failure code instead of halting.
/// The selftest binary sets this so hangs cannot masquerade as passes.
pub static EXIT_QEMU_ON_PANIC: AtomicBool = AtomicBool::new(false);

/// Early kernel bring-up (stages B010–B030): serial, identity, CPU baseline.
pub fn early_init(boot_info: &'static mut BootInfo) -> &'static mut BootInfo {
    serial::init();
    bootstage::emit(Stage::B010KernelEntry);
    bootstage::emit(Stage::B020SerialReady);

    serial_println!(
        "[ITISYOU:INFO] kernel=itisyou-os version={} arch=x86_64 boot=bootloader-{}.{}.{}",
        env!("CARGO_PKG_VERSION"),
        boot_info.api_version.version_major(),
        boot_info.api_version.version_minor(),
        boot_info.api_version.version_patch(),
    );
    serial_println!(
        "[ITISYOU:INFO] memory_regions={} physical_memory_offset={} framebuffer={}",
        boot_info.memory_regions.len(),
        match boot_info.physical_memory_offset.as_ref() {
            Some(offset) => {
                let _ = offset;
                "mapped"
            }
            None => "unmapped",
        },
        if boot_info.framebuffer.as_ref().is_some() {
            "present"
        } else {
            "absent"
        },
    );

    cpu::report_baseline();
    bootstage::emit(Stage::B030CpuBaseline);

    boot_info
}

/// Full subsystem bring-up (stages B040–B120), in the dependency order
/// defined by the implementation plan §20 Phase 2.
///
/// Panics on invariant violations (invalid memory map, heap init failure) —
/// continuing with known-corrupt foundations is prohibited (plan §11.2).
pub fn init_subsystems(boot_info: &'static mut BootInfo) {
    let phys_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader must map physical memory (config requests it)");

    // B040/B050: memory map validation + physical memory manager.
    let (map_stats, pmm_stats) =
        memory::init(&boot_info.memory_regions).expect("boot memory map validation failed");
    serial_println!(
        "[ITISYOU:INFO] memmap usable_frames={} reserved_frames={} highest_usable={:#x}",
        map_stats.usable_frames,
        map_stats.reserved_frames,
        map_stats.highest_usable_addr,
    );
    bootstage::emit(Stage::B040MemoryMapValidated);
    serial_println!(
        "[ITISYOU:INFO] pmm free_frames={} ignored_high_frames={}",
        map_stats.usable_frames - pmm_stats.ignored_high_frames,
        pmm_stats.ignored_high_frames,
    );
    bootstage::emit(Stage::B050PhysicalMemoryReady);

    // B060: paging abstraction over the bootloader-provided mapping.
    // SAFETY: phys_offset is the bootloader's physical-memory mapping and
    // this is the single initialization call.
    unsafe { memory::paging::init(phys_offset) };
    bootstage::emit(Stage::B060VirtualMemoryReady);

    // B070: kernel heap.
    memory::heap::init().expect("kernel heap initialization failed");
    bootstage::emit(Stage::B070HeapReady);

    // B080: GDT/TSS/IDT + exception handlers.
    interrupts::init_descriptors();
    // With our own descriptors live, the bootloader's low identity mappings
    // are dead — unlink them so L4 entry 0 belongs to processes alone.
    memory::paging::release_boot_identity_mappings();
    bootstage::emit(Stage::B080DescriptorsReady);

    // B090: PIC remap + PIT timer + interrupts on.
    interrupts::enable_timer();
    // Calibrate the TSC against the now-running PIT. It is the only clock
    // that keeps advancing inside a syscall, where SFMASK has cleared IF
    // and the timer ISR cannot run.
    interrupts::calibrate_tsc();
    bootstage::emit(Stage::B090InterruptTimerReady);

    // B100: scheduler (boot context becomes task 0).
    task::init();
    bootstage::emit(Stage::B100SchedulerReady);

    // B110: VFS + embedded initramfs.
    let (files, dirs) = fs::init();
    serial_println!("[ITISYOU:INFO] initramfs files={files} dirs={dirs}");
    bootstage::emit(Stage::B110VfsReady);

    // B120: input path (polled serial RX).
    shell::init_input();

    // B140: Ring 3 transition machinery (user GDT segments were installed
    // at B080; this arms the syscall MSRs and kernel syscall stack).
    // CPU-enforced kernel/user separation, before any Ring 3 process can
    // exist: enabling SMAP while a syscall was mid-copy would fault the kernel
    // on its own legitimate access.
    harden::init();
    syscall::init();
    proc::init();
    ipc::init();
    bootstage::emit(Stage::B140UserspaceReady);

    // B160: device layer — PCI enumeration + storage discovery (V0.3).
    let pci_devices = device::pci::scan_and_report();
    if let Some(storage) = device::pci::find_storage(&pci_devices) {
        serial_println!(
            "[ITISYOU:INFO] storage_controller found bus={} slot={} class={:#04x} subclass={:#04x} name=\"{}\"",
            storage.bus,
            storage.slot,
            storage.id.class,
            storage.id.subclass,
            storage.id.class_name(),
        );
    }
    bootstage::emit(Stage::B160StorageReady);

    // B170: graphics — wrap the bootloader framebuffer + compositor (V0.5).
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        if let Some(gi) = gfx::init(fb) {
            serial_println!(
                "[ITISYOU:INFO] gfx width={} height={} bpp={} bgr={}",
                gi.width,
                gi.height,
                gi.bytes_per_pixel,
                gi.bgr,
            );
            gfx::compositor::init();
            bootstage::emit(Stage::B170GraphicsReady);

            // B180: PS/2 input + initial desktop composite.
            input::init();
            gfx::compositor::composite();
            bootstage::emit(Stage::B180DesktopReady);
        }
    }

    // B190: device model — enumerate PCI into Device records (sized BARs +
    // capability lists), report them, and bind registered drivers (V0.6).
    let devices = device::init();
    serial_println!("[ITISYOU:INFO] devmodel_ready devices={devices}");
    bootstage::emit(Stage::B190DeviceModelReady);

    // B200: network interface. The NIC is bound by the device model above;
    // this applies the address plan and marks the stack ready. Emitted with
    // `nic=absent` when no card is attached so a test can tell "no NIC in this
    // guest" apart from "the stage never ran".
    net::init();
    serial_println!(
        "[ITISYOU:INFO] network_ready nic={}",
        if device::e1000::present() {
            "e1000"
        } else {
            "absent"
        }
    );
    bootstage::emit(Stage::B200NetworkReady);

    // B210: interrupt modernization. ACPI first (V0.9): the MADT says where the
    // I/O APIC is and which GSI each ISA IRQ arrives on. Then the local APIC,
    // then the cutover — timer, keyboard and mouse move to the I/O APIC and the
    // 8259s are retired (ADR-0019). Without a MADT the cutover is refused and
    // the verified PIC path stays in charge.
    acpi::init(boot_info.rsdp_addr.into_option());
    if apic::init() {
        apic::cutover_legacy_irqs();
        let armed = device::with_devices(|devices| {
            devices
                .iter()
                .find(|d| d.driver == Some("e1000"))
                .map(|d| apic::enable_msi(d, interrupts::VECTOR_MSI))
                .unwrap_or(false)
        });
        if armed {
            device::e1000::with(|nic| nic.enable_interrupts());
        }
        // The 82540EM this machine model provides exposes no MSI capability,
        // so `nic_msi=false` is the expected state here rather than a failure.
        // Message-signalled delivery is proved on the NVMe controller's MSI-X
        // instead (`irq` command); saying which device carries the proof beats
        // a bare "MSI supported".
        let msix_capable = device::with_devices(|devices| {
            devices
                .iter()
                .filter(|d| d.has_cap(kernel_core::pci::CapabilityId::MsiX))
                .count()
        });
        serial_println!(
            "[ITISYOU:IRQ] apic_ready nic_msi={armed} msix_capable_devices={msix_capable}"
        );
    }
    bootstage::emit(Stage::B210ApicReady);

    // Provenance spans reboots: if a durable trail is present, verify it and
    // continue its hash chain. A missing trail is a first boot, not a fault,
    // and a tampered one is reported rather than quietly replaced.
    audit::recover();
}

/// Enumerate PCI, find the NVMe controller, and initialize it. Used by the
/// filesystem-persistence boot binary.
pub fn open_nvme() -> Option<device::nvme::Nvme> {
    let devices = device::pci::enumerate(0);
    let dev = device::pci::find_nvme(&devices)?;
    device::nvme::Nvme::init(&dev).ok()
}

/// The persistent store's namespace prefix. A path under it addresses the
/// ITFS volume on NVMe; anything else is the read-only initramfs.
pub const STORE_PREFIX: &str = "/data/";

/// Mount the persistent ITFS and run `f` over it.
///
/// A blank disk is formatted on first use, but a disk that mounts as
/// *corrupt* is NOT: reformatting on a bad CRC would turn a recoverable
/// read error into data loss. Only `NoValidSuperblock`/`BadMagic` — the
/// signatures of a disk that never held a filesystem — lead to a format.
pub fn with_persistent_store<R>(f: impl FnOnce(&mut fs_disk::FileSystem) -> R) -> Option<R> {
    let nvme = open_nvme()?;
    let mut fs = match fs_disk::FileSystem::mount(&nvme) {
        Ok(fs) => fs,
        Err(fs_disk::Error::Fs(
            kernel_core::itfs::FsError::NoValidSuperblock | kernel_core::itfs::FsError::BadMagic,
        )) => fs_disk::FileSystem::format(&nvme).ok()?,
        Err(_) => return None,
    };
    Some(f(&mut fs))
}

/// Mount the persistent store WITHOUT ever formatting it, and run `f`.
///
/// Boot-time readers must use this. `with_persistent_store` formats a blank
/// disk on first use, which is right for an explicit write and wrong for a
/// passive read: a guest attached to a disposable test disk that is not an
/// ITFS volume would have it reformatted just by booting, destroying whatever
/// the disk was actually for.
pub fn with_mounted_store<R>(f: impl FnOnce(&mut fs_disk::FileSystem) -> R) -> Option<R> {
    let nvme = open_nvme()?;
    let mut fs = fs_disk::FileSystem::mount(&nvme).ok()?;
    Some(f(&mut fs))
}

/// Split a `/data/<name>` path into its store-relative name.
///
/// Returns `None` for anything outside the store, for an empty name, for a
/// name with a further `/` in it (ITFS has no directories, and silently
/// flattening `a/b` to one name would let two different paths collide), or
/// for a name longer than ITFS allows.
pub fn store_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix(STORE_PREFIX)?;
    if name.is_empty() || name.len() > kernel_core::itfs::NAME_LEN || name.contains('/') {
        return None;
    }
    Some(name)
}

/// Filesystem-persistence boot logic (invoked by `itisyou-fs-persist`): if a
/// valid ITFS with `name` already exists, verify its contents (post-reboot
/// run); otherwise format and write it (first run). Never returns — exits
/// QEMU with success/failure.
pub fn run_fs_persist(name: &str, content: &[u8]) -> ! {
    use fs_disk::FileSystem;
    let Some(nvme) = open_nvme() else {
        serial_println!("[ITISYOU:INFO] fs_persist nvme=absent");
        qemu::exit(qemu::ExitCode::Failed);
    };

    // A valid ITFS with our file already present → verify (second boot).
    if let Ok(fs) = FileSystem::mount(&nvme) {
        if let Ok(data) = fs.read(name) {
            if data == content {
                serial_println!(
                    "[ITISYOU:INFO] FS-PERSIST-VERIFIED file={name} bytes={}",
                    data.len()
                );
                qemu::exit(qemu::ExitCode::Success);
            }
            serial_println!("[ITISYOU:INFO] FS-PERSIST-MISMATCH");
            qemu::exit(qemu::ExitCode::Failed);
        }
    }

    // No filesystem yet → format and write (first boot).
    match (|| -> Result<(), fs_disk::Error> {
        let mut fs = FileSystem::format(&nvme)?;
        fs.create(name, content)?;
        Ok(())
    })() {
        Ok(()) => {
            serial_println!(
                "[ITISYOU:INFO] FS-PERSIST-WROTE file={name} bytes={}",
                content.len()
            );
            qemu::exit(qemu::ExitCode::Success);
        }
        Err(e) => {
            serial_println!("[ITISYOU:INFO] FS-PERSIST-WRITE-FAILED err={e:?}");
            qemu::exit(qemu::ExitCode::Failed);
        }
    }
}

/// Kernel panic handler: emit a machine-readable marker plus diagnostics on a
/// freshly re-initialized serial port (bypassing any lock a panicking context
/// may already hold), then either exit QEMU (test mode) or halt forever.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use core::fmt::Write;
    x86_64::instructions::interrupts::disable();
    // SAFETY: the panic path must not deadlock on the global serial lock, so
    // it re-creates the UART driver directly. Output interleaving is
    // acceptable during a panic; correctness of the port setup is guaranteed
    // by re-running init.
    let mut port = unsafe { uart_16550::SerialPort::new(serial::COM1_PORT) };
    port.init();
    // Single-line marker (the machine-readable contract requires one line):
    // PanicInfo's Display splits location and message across lines.
    match info.location() {
        Some(location) => {
            let _ = writeln!(port, "[ITISYOU:PANIC] {} at {location}", info.message());
        }
        None => {
            let _ = writeln!(port, "[ITISYOU:PANIC] {}", info.message());
        }
    }
    if EXIT_QEMU_ON_PANIC.load(Ordering::SeqCst) {
        qemu::exit(qemu::ExitCode::Failed);
    }
    qemu::halt_loop();
}
