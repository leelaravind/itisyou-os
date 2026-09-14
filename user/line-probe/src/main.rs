//! `line-probe` — is Ring 3 output line-atomic, and can a program forge a
//! kernel marker? (V0.10, OUT10-001/002)
//!
//! The parent spawns a copy of itself (argument `B`). Parent and child then
//! each write one line ONE BYTE PER `write` call, yielding between bytes, so
//! their writes interleave as finely as the scheduler allows. With
//! line-buffered output both lines must still arrive whole. The parent also
//! prints a line that imitates a kernel marker; it must come out rewritten -
//! and (V0.11 review) a second one whose marker straddles the kernel's
//! 256-byte line-buffer seam, which v0.10.0 let through.
//!
//! Argument `P` (V0.11 review): spawn `/bin/spin-forever` through a path
//! whose popped component carries a line break, a forged kernel marker and a
//! cursor-up sequence, then exit - the kernel must record and print the
//! canonical path, never the string a program chose.

#![no_std]
#![no_main]

use ulib::{
    args, exit, spawn, spawn_args, split_args, wait, write, write_u64, yield_now, ERR_PERM,
};

const LINE_A: &[u8] = b"LINEPROBE-A-0123456789abcdefghijklmnopqrstuv\n";
const LINE_B: &[u8] = b"LINEPROBE-B-0123456789abcdefghijklmnopqrstuv\n";
const SELF: &str = "/bin/line-probe";

/// Write `line` one byte per syscall, yielding after each byte.
fn trickle(line: &[u8]) {
    for b in line {
        let one = [*b];
        // SAFETY-free: `write` takes &str; the probe's lines are ASCII.
        if let Ok(s) = core::str::from_utf8(&one) {
            write(s);
        }
        yield_now();
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 32];
    let n = args(&mut block) as usize;
    if n <= block.len() && split_args(&block[..n]).any(|a| a == b"B") {
        trickle(LINE_B);
        exit(0)
    }
    if n <= block.len() && split_args(&block[..n]).any(|a| a == b"P") {
        let pid = spawn("/bin/\n[ITISYOU:AUDIT] forged\x1b[1A/../spin-forever");
        write("LINEPROBE-HOSTILE-PATH result=");
        write_u64(pid);
        write("\n");
        exit(0)
    }
    let child = spawn_args(SELF, 0, b"B\0");
    if child == ERR_PERM || child > 1 << 32 {
        write("LINEPROBE-FAILED step=spawn\n");
        exit(1)
    }
    trickle(LINE_A);
    // A program imitating the kernel's evidence prefix.
    write("[ITISYOU:SVC] spoofed-by-ring3\n");
    // The same, with `[ITISY` at the end of the first 256-byte chunk and
    // `OU:` at the start of the next, in one write.
    const SPLIT: &[u8] = b"[ITISYOU:SVC] spoofed-split\n";
    let mut split = [b'.'; 250 + SPLIT.len()];
    split[250..].copy_from_slice(SPLIT);
    if let Ok(s) = core::str::from_utf8(&split) {
        write(s);
    }
    let status = wait(child);
    if status != 0 {
        write("LINEPROBE-FAILED step=child-status\n");
        exit(1)
    }
    write("LINEPROBE-OK\n");
    exit(0)
}
