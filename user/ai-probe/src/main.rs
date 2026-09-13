//! `ai-probe` - drives the AI-native system layer's kernel interfaces from
//! Ring 3 (V0.11, ADR-0024) and prints what the kernel answered. The first
//! argument picks the mode:
//!
//! * `view` - read the approved view (`sys_view`), decode it with the same
//!   host-tested `kernel_core::sysview` the agent uses, and print it:
//!   `AIPROBE-VIEW sched ...`, one `AIPROBE-VIEW service=...` per row, the
//!   audit counts and the 16 features. A buffer one byte too small must be
//!   refused (`ERR_2BIG`) before anything is copied.
//! * `view-denied` - run WITHOUT the view capability: the call must be
//!   refused (`AIPROBE-VIEW-DENIED err=perm`); if the kernel served a view
//!   anyway the probe prints `AIPROBE-VIEW-LEAK`, which the test forbids.

#![no_std]
#![no_main]

use kernel_core::sysview::{self, View};
use ulib::{args, exit, split_args, sys_view, write, write_u64, ERR_2BIG, ERR_PERM};

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

fn fail(step: &str, v: u64) -> ! {
    write("AIPROBE-FAILED step=");
    write(step);
    write(" err=");
    write_u64(u64::MAX - v);
    write("\n");
    exit(1)
}

fn view() {
    // One byte short: refused whole, nothing copied.
    let mut small = [0u8; sysview::LEN - 1];
    let r = sys_view(&mut small);
    if r != ERR_2BIG {
        fail("small-buffer", r);
    }
    if small.iter().any(|&b| b != 0) {
        write("AIPROBE-FAILED step=small-buffer-written\n");
        exit(1);
    }
    write("AIPROBE-VIEW-SMALL-OK\n");

    let mut buf = [0u8; sysview::LEN + 8];
    let n = sys_view(&mut buf);
    if is_err(n) {
        fail("sys_view", n);
    }
    if n as usize != sysview::LEN {
        write("AIPROBE-FAILED step=length\n");
        exit(1);
    }
    let v = match View::decode(&buf[..sysview::LEN]) {
        Ok(v) => v,
        Err(e) => {
            write("AIPROBE-VIEW-DECODE-FAILED reason=");
            write(e.name());
            write("\n");
            exit(1)
        }
    };
    write("AIPROBE-VIEW sched paused=");
    write(flag(v.paused));
    write(" always_on=");
    write(flag(v.always_on));
    write(" processes=");
    write_u64(u64::from(v.processes));
    write(" runnable=");
    write_u64(u64::from(v.runnable));
    write(" waiting=");
    write_u64(u64::from(v.waiting));
    write(" ended=");
    write_u64(u64::from(v.ended));
    write(" truncated=");
    write(flag(v.truncated));
    write(" \n");
    for row in v.rows() {
        write("AIPROBE-VIEW service=");
        write(row.name_str());
        write(" state=");
        write(row.state.name());
        write(" restarts=");
        write_u64(u64::from(row.restarts));
        write(" by_init=");
        write(flag(row.by_init));
        write(" \n");
    }
    write("AIPROBE-VIEW audit records=");
    write_u64(u64::from(v.recent_records));
    write(" denials=");
    write_u64(u64::from(v.recent_denials));
    write(" \nAIPROBE-VIEW features=");
    for (i, f) in sysview::features(&v).iter().enumerate() {
        if i > 0 {
            write(",");
        }
        write_u64(*f as u64);
    }
    write("\nAIPROBE-VIEW-OK\n");
}

fn view_denied() {
    let mut buf = [0u8; sysview::LEN];
    let r = sys_view(&mut buf);
    if r == ERR_PERM {
        write("AIPROBE-VIEW-DENIED err=perm\n");
    } else if is_err(r) {
        fail("view-denied", r);
    } else {
        write("AIPROBE-VIEW-LEAK\n");
        exit(1);
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 128];
    let len = args(&mut block);
    let mode = if is_err(len) {
        None
    } else {
        split_args(&block[..len as usize]).next()
    };
    match mode {
        Some(b"view") => view(),
        Some(b"view-denied") => view_denied(),
        _ => {
            write("AIPROBE-FAILED step=mode\n");
            exit(2)
        }
    }
    exit(0)
}
