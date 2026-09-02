//! Selftest kernel binary: full boot, in-kernel verification suite,
//! deterministic QEMU exit. Used by the automated boot/regression tests.

#![no_std]
#![no_main]

use bootloader_api::{entry_point, BootInfo};
use core::sync::atomic::Ordering;
use itisyou_kernel as kernel;

entry_point!(kernel_main, config = &kernel::BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // A panic during selftest must fail the QEMU run, not hang it.
    kernel::EXIT_QEMU_ON_PANIC.store(true, Ordering::SeqCst);

    let boot_info = kernel::early_init(boot_info);

    let mut suite = kernel::selftest::Suite::new();
    // Boot-handoff checks before full init consumes the boot info.
    suite.check("memory_map_nonempty", !boot_info.memory_regions.is_empty());
    suite.check(
        "physical_memory_mapped",
        boot_info.physical_memory_offset.as_ref().is_some(),
    );

    kernel::init_subsystems(boot_info);
    kernel::serial_println!("[ITISYOU:MODE] selftest");

    kernel::selftest::run_all(&mut suite);

    kernel::serial_println!("[ITISYOU:SELFTEST] pass={} fail={}", suite.pass, suite.fail);
    if suite.fail == 0 {
        kernel::qemu::exit(kernel::qemu::ExitCode::Success);
    }
    kernel::qemu::exit(kernel::qemu::ExitCode::Failed);
}
