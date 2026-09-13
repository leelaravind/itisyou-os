//! `proc-probe` — the V0.10 process model from the inside (PROC10-002).
//!
//! The first argument picks what to check:
//!
//! * `all` — a sibling's attempt to collect our child, then everything below
//!   in order, then spawns an orphan and exits WITHOUT waiting for it (the
//!   kernel must reap it);
//! * `foreign <pid>` — `wait` and `wait_nohang` on a process that is not our
//!   child must be refused (`ERR_NOENT`), not block and not collect it;
//! * `nohang` — `wait_nohang`: no children, a running child (`ERR_AGAIN`),
//!   collecting it, and collecting "any child";
//! * `sleep` — `sleep(50)` lasts at least 50 ticks; `sleep(6001)` is refused;
//! * `orphan` — spawn an orphan and exit at once;
//! * `orphan-child` — (spawned by `orphan`) sleep 20 ticks, report, exit 0.
//!
//! Every check prints a `PROCPROBE-…` marker; a failure prints
//! `PROCPROBE-FAILED step=<step>` and exits 1.

#![no_std]
#![no_main]

use ulib::{
    args, exit, sleep_ticks, spawn, spawn_args, split_args, uptime_ticks, wait, wait_nohang, write,
    write_u64, yield_now, CAP_SPAWN, ERR_AGAIN, ERR_INVAL, ERR_NOENT,
};

const SELF: &str = "/bin/proc-probe";
/// `/bin/child` exits with this status.
const CHILD_STATUS: u64 = 7;
/// Bound on every polling loop, so a broken kernel fails the probe instead
/// of hanging it.
const BUDGET: u32 = 200_000;

fn fail(step: &str) -> ! {
    write("PROCPROBE-FAILED step=");
    write(step);
    write("\n");
    exit(1)
}

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn parse_u64(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 19 {
        return None;
    }
    let mut v = 0u64;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u64::from(c - b'0');
    }
    Some(v)
}

fn foreign(pid: u64) {
    if wait(pid) != ERR_NOENT {
        fail("foreign-wait");
    }
    write("PROCPROBE-FOREIGN-WAIT-REFUSED pid=");
    write_u64(pid);
    write("\n");
    let mut status = 0u64;
    if wait_nohang(pid, &mut status) != ERR_NOENT {
        fail("foreign-nohang");
    }
    write("PROCPROBE-FOREIGN-NOHANG-REFUSED pid=");
    write_u64(pid);
    write("\n");
}

/// Poll `wait_nohang(pid)` until it collects `expect`.
fn collect(pid: u64, expect: u64, step: &str) -> u64 {
    let mut status = 0u64;
    for _ in 0..BUDGET {
        let r = wait_nohang(pid, &mut status);
        if r == expect {
            return status;
        }
        if r != ERR_AGAIN {
            fail(step);
        }
        yield_now();
    }
    fail(step)
}

fn nohang() {
    let mut status = 0u64;
    if wait_nohang(0, &mut status) != ERR_NOENT {
        fail("nochild");
    }
    write("PROCPROBE-NOCHILD-OK\n");

    let c = spawn("/bin/child");
    if is_err(c) {
        fail("spawn-child");
    }
    // The child cannot have finished: it yields three times before it
    // exits, and round-robin runs this probe between any two of its quanta.
    if wait_nohang(c, &mut status) != ERR_AGAIN {
        fail("nohang-again");
    }
    write("PROCPROBE-NOHANG-AGAIN-OK\n");
    let st = collect(c, c, "nohang-reap");
    write("PROCPROBE-NOHANG-REAPED status=");
    write_u64(st);
    write("\n");
    if st != CHILD_STATUS {
        fail("nohang-status");
    }
    // Collected means gone: a second collection finds nothing.
    if wait_nohang(c, &mut status) != ERR_NOENT {
        fail("nohang-twice");
    }

    let c2 = spawn("/bin/child");
    if is_err(c2) {
        fail("spawn-child-2");
    }
    let st = collect(0, c2, "any-reap");
    write("PROCPROBE-ANY-REAPED status=");
    write_u64(st);
    write("\n");
    if st != CHILD_STATUS {
        fail("any-status");
    }
}

fn sleep_check() {
    let t0 = uptime_ticks();
    if sleep_ticks(50) != 0 {
        fail("sleep");
    }
    let elapsed = uptime_ticks() - t0;
    if elapsed >= 50 {
        write("PROCPROBE-SLEEP-OK requested=50\n");
    } else {
        write("PROCPROBE-SLEEP-SHORT elapsed=");
        write_u64(elapsed);
        write("\n");
        exit(1)
    }
    if sleep_ticks(6001) != ERR_INVAL {
        fail("sleep-bound");
    }
    write("PROCPROBE-SLEEP-BOUND-OK\n");
}

/// Decimal digits of `v` into `out`; returns how many.
fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for (o, d) in out.iter_mut().zip(digits[..n].iter().rev()) {
        *o = *d;
    }
    n
}

/// A SIBLING must not be able to collect our child: spawn a target, then a
/// second probe (holding only the Process capability) told to collect it.
/// The sibling must be refused both ways and exit 0, and the target must
/// still be ours to collect afterwards, with its own status.
fn sibling() {
    let target = spawn("/bin/child");
    if is_err(target) {
        fail("spawn-target");
    }
    let mut block = [0u8; 40];
    let prefix = b"foreign\0";
    block[..prefix.len()].copy_from_slice(prefix);
    let mut len = prefix.len();
    len += fmt_u64(target, &mut block[len..]);
    block[len] = 0;
    len += 1;
    let sib = spawn_args(SELF, CAP_SPAWN, &block[..len]);
    if is_err(sib) {
        fail("spawn-sibling");
    }
    if collect(sib, sib, "sibling-reap") != 0 {
        fail("sibling-status");
    }
    if collect(target, target, "target-reap") != CHILD_STATUS {
        fail("target-status");
    }
    write("PROCPROBE-SIBLING-REFUSED-OK\n");
}

fn orphan() {
    let c = spawn_args(SELF, 0, b"orphan-child\0");
    if is_err(c) {
        fail("spawn-orphan");
    }
    write("PROCPROBE-ORPHAN-SPAWNED\n");
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; 64];
    let n = args(&mut buf);
    let block: &[u8] = if n as usize <= buf.len() {
        &buf[..n as usize]
    } else {
        &[]
    };
    let mut it = split_args(block);
    match it.next() {
        Some(b"all") => {
            sibling();
            nohang();
            sleep_check();
            orphan();
        }
        Some(b"foreign") => match it.next().and_then(parse_u64) {
            Some(pid) => foreign(pid),
            None => fail("foreign-usage"),
        },
        Some(b"nohang") => nohang(),
        Some(b"sleep") => sleep_check(),
        Some(b"orphan") => orphan(),
        Some(b"orphan-child") => {
            if sleep_ticks(20) != 0 {
                fail("orphan-sleep");
            }
            write("PROCPROBE-ORPHAN-CHILD-DONE\n");
            exit(0)
        }
        _ => {
            write("PROCPROBE-USAGE all|foreign <pid>|nohang|sleep|orphan\n");
            exit(2)
        }
    }
    write("PROCPROBE-OK\n");
    exit(0)
}
