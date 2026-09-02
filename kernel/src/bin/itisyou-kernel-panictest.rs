//! Negative-path verification binary: boots minimally, then panics on
//! purpose. The harness (`--expect-panic`) asserts that the panic path
//! produces its machine-readable serial evidence (requirement DIAG-001).

#![no_std]
#![no_main]

use bootloader_api::{entry_point, BootInfo};
use itisyou_kernel as kernel;

entry_point!(kernel_main, config = &kernel::BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::early_init(boot_info);
    kernel::serial_println!("[ITISYOU:MODE] panictest");
    panic!("intentional panic-test (negative-path verification)");
}
