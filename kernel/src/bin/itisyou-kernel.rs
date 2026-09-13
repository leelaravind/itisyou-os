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

    // V0.8: bring up the persistent Ring 3 services before handing the console
    // to the shell. They are started here rather than in `init_subsystems` so
    // the selftest binary keeps a quiescent, deterministic process table —
    // background daemons would otherwise be scheduled in the middle of the
    // scheduler and userspace tests.
    kernel::services::start_background();
    // V0.10 (SCHED10-001; ADR-0022): background processes run in bounded
    // slices at the audited safe points from here on. Never in the selftest
    // image, whose checks count every process step.
    kernel::sched::enable();

    kernel::bootstage::emit(Stage::B130ShellRunning);
    // All V0.1 hard-target boot stages are up: acceptance marker.
    kernel::bootstage::emit(Stage::B150Acceptance);
    kernel::shell::run()
}
