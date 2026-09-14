//! `soak` - a repeatable workload for the leak check (V1.0, V1-REL-003).
//!
//! `soak <n>` runs `n` iterations of what a long-lived system does over and
//! over: start a child and collect it (a whole process lifecycle: address
//! space, stacks, capability handles, output buffer), write a file and
//! delete it, and ask `tickd` for its pass count over IPC. Every iteration
//! must succeed; the console's `leakcheck` then shows whether any of it left
//! something behind in the kernel.
//!
//! Needs `spawn`, `ipc:2-3` (tickd's channels), `fs_write` and a `/data`
//! sandbox. Prints `SOAK-OK iterations=<n> lifecycles=<n>` or
//! `SOAK-FAILED step=<step> iteration=<i>`.

#![no_std]
#![no_main]

use ulib::{
    args, exit, fs_delete, fs_write, msg_recv, msg_send, spawn, split_args, wait, write, write_u64,
    yield_now, ERR_AGAIN,
};

/// `/bin/child` exits with this status.
const CHILD_STATUS: u64 = 7;
const TICK_REQ: u64 = 2;
const TICK_REP: u64 = 3;
const FILE: &str = "/data/soak.tmp";
/// Bound on each IPC wait, so a missing service fails instead of hanging.
const BUDGET: u32 = 200_000;
const MAX_ITERATIONS: u64 = 100_000;

fn fail(step: &str, i: u64) -> ! {
    write("SOAK-FAILED step=");
    write(step);
    write(" iteration=");
    write_u64(i);
    write("\n");
    exit(1)
}

fn parse(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 6 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(b.iter().fold(0, |n, d| n * 10 + u64::from(d - b'0')))
}

fn tick_round_trip() -> bool {
    let mut budget = BUDGET;
    while msg_send(TICK_REQ, b"stat") == ERR_AGAIN {
        yield_now();
        budget -= 1;
        if budget == 0 {
            return false;
        }
    }
    let mut buf = [0u8; 8];
    loop {
        match msg_recv(TICK_REP, &mut buf) {
            8 => return u64::from_le_bytes(buf) > 0,
            ERR_AGAIN => {}
            _ => return false,
        }
        yield_now();
        budget -= 1;
        if budget == 0 {
            return false;
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 32];
    let len = args(&mut block) as usize;
    let mut words = split_args(&block[..len.min(block.len())]);
    let n = words
        .next()
        .and_then(parse)
        .filter(|&n| (1..=MAX_ITERATIONS).contains(&n))
        .unwrap_or_else(|| fail("args", 0));
    // Optional second word: which activities to run - any of `s` (spawn),
    // `f` (file) and `i` (IPC); all three by default.
    let only = words.next().unwrap_or(b"sfi");
    let (do_spawn, do_file, do_ipc) = (
        only.contains(&b's'),
        only.contains(&b'f'),
        only.contains(&b'i'),
    );
    for i in 1..=n {
        if do_spawn {
            let child = spawn("/bin/child");
            if child > u64::MAX - 16 {
                fail("spawn", i);
            }
            if wait(child) != CHILD_STATUS {
                fail("wait", i);
            }
        }
        if do_file {
            if fs_write(FILE, b"soak") != 4 {
                fail("fs_write", i);
            }
            if fs_delete(FILE) != 0 {
                fail("fs_delete", i);
            }
        }
        if do_ipc && !tick_round_trip() {
            fail("ipc", i);
        }
    }
    write("SOAK-OK iterations=");
    write_u64(n);
    write(" lifecycles=");
    write_u64(n);
    write("\n");
    exit(0)
}
