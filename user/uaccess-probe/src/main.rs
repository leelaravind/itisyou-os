//! `uaccess-probe` — can a program make the kernel write into memory the
//! program itself may not write? (V0.10, SEC10-001)
//!
//! It hands two syscalls that need NO capability — `args` and `cap_list` — a
//! pointer into its own read-only code and its own read-only data. The
//! kernel must refuse with `ERR_FAULT`. Before V0.10 the kernel checked only
//! that the pages were mapped, wrote through them in Ring 0, and the
//! write-protect fault that followed panicked the whole kernel: any program,
//! holding nothing, could stop the machine.
//!
//! Run it WITH at least one argument (`run /bin/uaccess-probe - - -- probe`):
//! with no arguments `args` has nothing to copy and never touches the buffer.

#![no_std]
#![no_main]

use ulib::{args, exit, raw_syscall, write, ERR_FAULT, SYS_ARGS, SYS_CAP_LIST};

/// Read-only data the probe points the kernel at (never written by anyone).
static RODATA: [u8; 64] = [0x5A; 64];

fn fail(step: &str) -> ! {
    write("UACCESS-FAILED step=");
    write(step);
    write("\n");
    exit(1)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // A writable buffer first: the program must actually have an argument,
    // or the read-only case below would prove nothing.
    let mut buf = [0u8; 64];
    let n = args(&mut buf);
    if n == 0 || n > buf.len() as u64 {
        fail("needs-an-argument");
    }

    // 1. `args` into the program's own code page (read-only, executable).
    let code = _start as *const () as usize as u64;
    if raw_syscall(SYS_ARGS, code, 512, 0) != ERR_FAULT {
        fail("args-code");
    }
    write("UACCESS-ARGS-RO-REFUSED\n");

    // 2. `cap_list` into the program's own read-only data.
    let rodata = core::hint::black_box(RODATA.as_ptr()) as u64;
    if raw_syscall(SYS_CAP_LIST, rodata, 4, 0) != ERR_FAULT {
        fail("caplist-rodata");
    }
    write("UACCESS-CAPLIST-RO-REFUSED\n");

    // 3. The same calls into writable memory still work.
    let again = args(&mut buf);
    let mut handles = [0u64; 4];
    let listed = raw_syscall(SYS_CAP_LIST, handles.as_mut_ptr() as u64, 4, 0);
    if again != n || listed > 4 {
        fail("writable");
    }
    write("UACCESS-RW-OK\n");
    write("UACCESS-OK\n");
    exit(0)
}
