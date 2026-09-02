//! ITISYOU OS kernel library.
//!
//! The two boot binaries (`itisyou-kernel`, `itisyou-kernel-selftest`) are
//! thin wrappers over this library. All subsystem logic lives here; pure
//! algorithmic logic that can run on the host lives in `kernel-core`.

#![no_std]

pub mod bootstage;
pub mod cpu;
pub mod qemu;
pub mod serial;

use bootloader_api::config::Mapping;
use bootloader_api::{BootInfo, BootloaderConfig};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use kernel_core::stage::Stage;

/// Bootloader handoff configuration shared by both boot binaries.
///
/// The full physical address space is mapped at a dynamic offset so the
/// physical memory manager can inspect frames without ad-hoc identity maps.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.kernel_stack_size = 128 * 1024;
    config
};

/// When set, a panic exits QEMU with a failure code instead of halting.
/// The selftest binary sets this so hangs cannot masquerade as passes.
pub static EXIT_QEMU_ON_PANIC: AtomicBool = AtomicBool::new(false);

/// Early kernel bring-up shared by both binaries.
///
/// Emits boot stages B010–B030 and returns the boot info for the caller to
/// continue initialization.
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

/// Kernel panic handler: emit a machine-readable marker plus diagnostics on a
/// freshly re-initialized serial port (bypassing any lock a panicking context
/// may already hold), then either exit QEMU (test mode) or halt forever.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use core::fmt::Write;
    // SAFETY: the panic path must not deadlock on the global serial lock, so
    // it re-creates the UART driver directly. Output interleaving is
    // acceptable during a panic; correctness of the port setup is guaranteed
    // by re-running init.
    let mut port = unsafe { uart_16550::SerialPort::new(serial::COM1_PORT) };
    port.init();
    let _ = writeln!(port, "[ITISYOU:PANIC] {info}");
    if EXIT_QEMU_ON_PANIC.load(Ordering::SeqCst) {
        qemu::exit(qemu::ExitCode::Failed);
    }
    qemu::halt_loop();
}
