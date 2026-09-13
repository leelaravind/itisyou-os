//! `burn` — a CPU-bound program that never yields (V0.10, SCHED10-001).
//!
//! It spins until the given number of timer ticks (default 150, at most
//! 6000) has passed, then prints `BURN-OK ticks=<n> yields=0`. It makes no
//! yield, wait or sleep call, so under a foreground `run` every other process
//! can get the CPU only when the timer preempts it — which is exactly what
//! the always-on scheduler has to turn into background progress.

#![no_std]
#![no_main]

use ulib::{args, exit, split_args, uptime_ticks, write, write_u64, ARGS_BLOCK_MAX};

const DEFAULT_TICKS: u64 = 150;
const MAX_TICKS: u64 = 6000;

fn parse(arg: &[u8]) -> Option<u64> {
    if arg.is_empty() || arg.len() > 5 {
        return None;
    }
    let mut v = 0u64;
    for &c in arg {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u64::from(c - b'0');
    }
    Some(v)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; ARGS_BLOCK_MAX];
    let len = args(&mut buf);
    let block: &[u8] = if len as usize <= ARGS_BLOCK_MAX {
        &buf[..len as usize]
    } else {
        &[]
    };
    let ticks = match split_args(block).next() {
        None => DEFAULT_TICKS,
        Some(a) => match parse(a) {
            Some(t) if (1..=MAX_TICKS).contains(&t) => t,
            _ => {
                write("BURN-BAD-ARG\n");
                exit(2)
            }
        },
    };
    let start = uptime_ticks();
    let mut spins = 0u64;
    while uptime_ticks().wrapping_sub(start) < ticks {
        spins = spins.wrapping_add(1);
        core::hint::spin_loop();
    }
    core::hint::black_box(spins);
    write("BURN-OK ticks=");
    write_u64(ticks);
    write(" yields=0\n");
    exit(0)
}
