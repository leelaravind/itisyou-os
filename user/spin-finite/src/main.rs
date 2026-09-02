//! A finite CPU-bound process that makes NO syscalls during its work loop —
//! so any interleaving with a sibling can only come from timer preemption.
//!
//! It sums 0..N in a register (result is only correct if registers survive
//! every preemption) and, each iteration, verifies a per-process sentinel on
//! its own stack (only correct if CR3/address-space isolation holds across
//! preemption). It exits 0 iff both invariants hold, else a distinct nonzero
//! code — turning silent corruption into a visible failure.

#![no_std]
#![no_main]

use core::hint::black_box;
use ulib::{exit, getpid, write, write_u64};

// ~40M iterations spans many 10 ms timer ticks under TCG, forcing dozens of
// preemptions during a run that contains zero syscalls.
const N: u64 = 40_000_000;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let pid = getpid();
    // Sentinel derived from pid, kept on this process's own stack.
    let sentinel: u64 = 0xF00D_0000 ^ pid.wrapping_mul(0x9E37_79B9);
    let mut guard = sentinel;

    let mut sum: u64 = 0;
    let mut i: u64 = 0;
    while i < N {
        sum = sum.wrapping_add(i);
        // Touch the stack sentinel every iteration; black_box stops the
        // optimizer from hoisting/eliding it so it really lives across
        // preemptions.
        guard = black_box(guard);
        i += 1;
    }

    let expected_sum = (N.wrapping_sub(1)).wrapping_mul(N) / 2;
    if guard != sentinel {
        write("SPIN-CORRUPT-SENTINEL\n");
        exit(2);
    }
    if black_box(sum) != expected_sum {
        write("SPIN-CORRUPT-SUM\n");
        exit(3);
    }
    write("SPIN-FINITE-OK pid=");
    write_u64(pid);
    write("\n");
    exit(0)
}
