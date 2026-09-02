//! ITISYOU OS kernel library.
//!
//! The boot binaries (`itisyou-kernel`, `itisyou-kernel-selftest`,
//! `itisyou-kernel-panictest`) are thin wrappers over this library. All
//! subsystem logic lives here; pure algorithmic logic that also runs on the
//! host lives in `kernel-core`.

#![no_std]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod bootstage;
pub mod cpu;
pub mod fs;
pub mod gdt;
pub mod interrupts;
pub mod memory;
pub mod qemu;
pub mod selftest;
pub mod serial;
pub mod shell;
pub mod task;

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
    bootstage::emit(Stage::B080DescriptorsReady);

    // B090: PIC remap + PIT timer + interrupts on.
    interrupts::enable_timer();
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
