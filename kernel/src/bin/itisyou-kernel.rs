//! Interactive kernel binary: normal boot path.

#![no_std]
#![no_main]

use bootloader_api::{entry_point, BootInfo};
use itisyou_kernel as kernel;

entry_point!(kernel_main, config = &kernel::BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::early_init(boot_info);
    kernel::serial_println!("[ITISYOU:MODE] interactive");
    // Later phases continue bring-up here (memory, interrupts, scheduler,
    // shell). The skeleton halts after early init.
    kernel::qemu::halt_loop();
}
