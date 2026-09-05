//! `harden-probe` — a Ring 3 program that tries to read kernel-only CPU state.
//!
//! `sgdt` stores the GDT's base address and limit. Without UMIP it is
//! unprivileged, so any userspace program can learn where the kernel's
//! descriptor tables live — a free kernel-address leak that defeats layout
//! randomisation before it exists. With UMIP the instruction raises #GP.
//!
//! The probe prints its intent, executes the instruction, and — if it is still
//! running afterwards — prints the address it just read, which is a leak. The
//! test asserts the fault marker is present AND that the leak marker is not:
//! "the CPU refused" has no line of its own, so absence is the only way to
//! state it.

#![no_std]
#![no_main]

use ulib::{exit, write, write_hex8};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("HARDEN-PROBE-SGDT\n");
    let mut descriptor = [0u8; 10];
    // SAFETY: `sgdt` writes 10 bytes to the given address. The buffer is that
    // size and lives on this program's own stack. Under UMIP the instruction
    // faults instead, which is the outcome being tested.
    unsafe {
        core::arch::asm!(
            "sgdt [{}]",
            in(reg) descriptor.as_mut_ptr(),
            options(nostack)
        );
    }
    // Reaching this line means the CPU allowed a Ring 3 program to read the
    // kernel's descriptor-table base.
    write("HARDEN-LEAK-SGDT base=");
    for byte in descriptor[2..10].iter().rev() {
        write_hex8(*byte);
    }
    write("\n");
    exit(1)
}
