//! `flags-probe` — does kernel code ever run with a Ring 3 program's DF/AC? (V0.10)
//!
//! A Ring 3 program may set the direction flag (DF) and the alignment-check
//! flag (AC). Neither is cleared by interrupt delivery. If kernel code then
//! runs with them set, string instructions run backwards (DF) and SMAP stops
//! protecting user memory (AC), so every path from Ring 3 into the kernel must
//! clear both before kernel code runs. Syscalls do (SFMASK); this probe
//! exercises the two paths that do not go through `syscall`:
//!
//! * default: set DF and AC and spin with no syscall long enough to be
//!   preempted by the timer many times;
//! * `fault`: set DF and AC and execute `hlt`, a #GP in Ring 3, so the fault
//!   path returns to the run-loop.
//!
//! The kernel counts every entry that finds DF or AC set; the console's
//! `harden` command reports the counters, and the QEMU leg requires them to
//! be zero after both runs (with a non-zero number of checks).

#![no_std]
#![no_main]

use core::arch::asm;
use ulib::{args, exit, split_args, write};

/// dec/jnz iterations with DF and AC set: several scheduler quanta under TCG.
const SPIN: u64 = 200_000_000;
/// RFLAGS.AC (bit 18).
const AC: u64 = 1 << 18;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 64];
    let n = args(&mut block) as usize;
    let fault = n <= block.len() && split_args(&block[..n]).any(|a| a == b"fault");
    if fault {
        write("FLAGSPROBE-FAULT-ARMED df=1 ac=1\n");
        // SAFETY: deliberately faulting; the kernel terminates the process.
        // Everything between setting the flags and `hlt` is asm, so no Rust
        // code runs with DF set.
        unsafe {
            asm!(
                "std",
                "pushfq",
                "or qword ptr [rsp], {ac}",
                "popfq",
                "hlt",
                ac = const AC,
                options(noreturn)
            );
        }
    }
    write("FLAGSPROBE-SPIN df=1 ac=1\n");
    // SAFETY: the flags are set and cleared inside this one asm block; no
    // Rust code (which assumes DF=0) runs while DF is set.
    unsafe {
        asm!(
            "std",
            "pushfq",
            "or qword ptr [rsp], {ac}",
            "popfq",
            "2:",
            "dec {n}",
            "jnz 2b",
            "pushfq",
            "and qword ptr [rsp], {not_ac}",
            "popfq",
            "cld",
            n = inout(reg) SPIN => _,
            ac = const AC,
            not_ac = const !AC as i64 as i32,
        );
    }
    write("FLAGSPROBE-SPUN\n");
    exit(0)
}
