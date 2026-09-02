//! `init` — the first userspace program. Every line below is deterministic
//! evidence asserted by the QEMU harness; nothing here can run unless the
//! CPU really is executing this ELF in Ring 3 and syscalls round-trip.

#![no_std]
#![no_main]

use ulib::{exit, getpid, raw_syscall, write, write_raw, yield_now, ERR_FAULT, ERR_NOSYS};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // 1. Basic syscall round-trip: user → kernel → user, output visible.
    write("RING3-HELLO\n");

    // 2. getpid returns a real pid.
    if getpid() >= 1 {
        write("RING3-PID-OK\n");
    }

    // 3. Unknown syscall numbers are rejected, process survives.
    if raw_syscall(999, 0, 0, 0) == ERR_NOSYS {
        write("RING3-EINVAL-OK\n");
    }

    // 4. Kernel-address pointer is rejected by validation, not dereferenced.
    if write_raw(0xFFFF_8000_0000_0000, 4) == ERR_FAULT {
        write("RING3-BADPTR-OK\n");
    }

    // 5. In-window but unmapped pointer is rejected too.
    if write_raw(0x0030_0000_0000, 4) == ERR_FAULT {
        write("RING3-UNMAPPED-OK\n");
    }

    // 6. yield parks in the kernel scheduler and returns.
    yield_now();
    write("RING3-YIELD-OK\n");

    // 7. Clean termination.
    write("RING3-DONE\n");
    exit(0)
}
