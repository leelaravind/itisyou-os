//! Interactive kernel binary: full boot to the serial shell.

#![no_std]
#![no_main]

use bootloader_api::{entry_point, BootInfo};
use itisyou_kernel as kernel;
use kernel_core::stage::Stage;

entry_point!(kernel_main, config = &kernel::BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    let boot_info = kernel::early_init(boot_info);
    kernel::init_subsystems(boot_info);
    kernel::serial_println!("[ITISYOU:MODE] interactive");

    kernel::bootstage::emit(Stage::B130ShellRunning);
    // All V0.1 hard-target boot stages are up: acceptance marker.
    kernel::bootstage::emit(Stage::B150Acceptance);
    kernel::shell::run()
}
