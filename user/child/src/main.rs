//! A child process spawned by `parent`. Prints its pid, yields a few times
//! (so the scheduler must interleave it with its sibling), then exits 7.

#![no_std]
#![no_main]

use ulib::{exit, getpid, write, write_u64, yield_now};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("RING3-CHILD-PID=");
    write_u64(getpid());
    write("\n");
    for _ in 0..3 {
        yield_now();
    }
    write("RING3-CHILD-EXIT pid=");
    write_u64(getpid());
    write("\n");
    exit(7)
}
