//! Executes `hlt` — a privileged instruction — from Ring 3. If we really
//! are at CPL=3 the CPU raises #GP and the kernel terminates this process;
//! the SURVIVED line must never appear in the serial log.

#![no_std]
#![no_main]

use ulib::{exit, write};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("RING3-GP-ATTEMPT\n");
    unsafe {
        core::arch::asm!("hlt", options(nostack, nomem));
    }
    write("RING3-GP-SURVIVED\n");
    exit(1)
}
