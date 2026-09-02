//! Selftest kernel binary: boots, runs in-kernel verification, exits QEMU
//! with a deterministic status. Used by the automated boot/regression suite.

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
    kernel::serial_println!("[ITISYOU:MODE] selftest");

    let mut pass: u32 = 0;
    let mut fail: u32 = 0;

    // Boot-skeleton selftests. Each emits a TEST event; the suite summary
    // decides the QEMU exit status.
    check(
        "memory_map_nonempty",
        !boot_info.memory_regions.is_empty(),
        &mut pass,
        &mut fail,
    );
    check(
        "physical_memory_mapped",
        boot_info.physical_memory_offset.as_ref().is_some(),
        &mut pass,
        &mut fail,
    );

    kernel::serial_println!("[ITISYOU:SELFTEST] pass={pass} fail={fail}");
    if fail == 0 {
        kernel::qemu::exit(kernel::qemu::ExitCode::Success);
    }
    kernel::qemu::exit(kernel::qemu::ExitCode::Failed);
}

fn check(name: &str, ok: bool, pass: &mut u32, fail: &mut u32) {
    if ok {
        *pass += 1;
        kernel::serial_println!("[ITISYOU:TEST] name={name} result=pass");
    } else {
        *fail += 1;
        kernel::serial_println!("[ITISYOU:TEST] name={name} result=fail");
    }
}
