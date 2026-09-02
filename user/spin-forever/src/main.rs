//! Adversarial process: spins forever with no syscalls, never yielding.
//! Under cooperative scheduling this would monopolize the CPU and starve
//! every sibling; under timer preemption it must not. The kernel proves
//! this by co-scheduling a finite process that still completes, then tearing
//! this one down.

#![no_std]
#![no_main]

use core::hint::black_box;
// Pull in ulib so its #[panic_handler] is linked even though this program
// never calls a ulib function on its normal path.
use ulib as _;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut x: u64 = 0;
    loop {
        x = black_box(x.wrapping_add(1));
    }
}
