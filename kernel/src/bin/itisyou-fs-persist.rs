//! Filesystem persistence proof (V0.4). Boots, brings up NVMe, and:
//!   - if a valid ITFS with `/hello` is already present → VERIFY its contents
//!     (this is the second boot, after a QEMU reboot on the same disk);
//!   - otherwise → FORMAT + write `/hello` and flush (the first boot).
//!
//! The automated test runs this binary TWICE against the SAME disposable
//! disk image (via the UEFI image — see docs/DEVELOPMENT_STORY for why the
//! BIOS image of this particular binary does not boot): run 1 writes, run 2
//! (a fresh QEMU guest) verifies — proving the data survived a full reboot
//! through the real block layer.

#![no_std]
#![no_main]

use bootloader_api::{entry_point, BootInfo};
use core::sync::atomic::Ordering;
use itisyou_kernel as kernel;

const FILE_NAME: &str = "hello";
const FILE_CONTENT: &[u8] = b"ITFS-PERSISTED-V04-2026";

entry_point!(kernel_main, config = &kernel::BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::EXIT_QEMU_ON_PANIC.store(true, Ordering::SeqCst);
    let boot_info = kernel::early_init(boot_info);
    kernel::init_subsystems(boot_info);
    kernel::serial_println!("[ITISYOU:MODE] fs-persist");
    kernel::run_fs_persist(FILE_NAME, FILE_CONTENT)
}
