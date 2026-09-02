//! Reads a kernel-half address from Ring 3. The MMU must fault (user code
//! cannot touch supervisor mappings); the kernel contains the #PF and
//! terminates this process. The SURVIVED line must never appear.

#![no_std]
#![no_main]

use ulib::{exit, write};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("RING3-PF-ATTEMPT\n");
    let kernel_addr = 0xFFFF_8000_DEAD_0000u64 as *const u64;
    let value = unsafe { core::ptr::read_volatile(kernel_addr) };
    // Use the value so the read cannot be optimized out.
    if value == 0x1234_5678 {
        write("RING3-PF-IMPROBABLE\n");
    }
    write("RING3-PF-SURVIVED\n");
    exit(1)
}
