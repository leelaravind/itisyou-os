//! `crashd` — a deliberately faulting service (V0.7). It announces itself,
//! then dereferences an unmapped address. The supervisor must contain the
//! fault, restart it up to the bounded limit, and then mark it Failed —
//! never a restart storm, never kernel damage.

#![no_std]
#![no_main]

use ulib::write;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("CRASHD-START\n");
    // Unmapped user address → #PF, contained by the kernel.
    // SAFETY: intentionally unsound — this crash IS the test.
    unsafe {
        core::ptr::read_volatile(0x10 as *const u8);
    }
    // Unreachable; loop to satisfy the never type without exiting cleanly.
    #[allow(clippy::empty_loop)]
    loop {}
}
