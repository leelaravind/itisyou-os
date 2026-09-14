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
//! * `wait-any` — `wait(0)` blocks until a running child ends and returns
//!   its status (V1.0, PROC1-001: it used to sleep forever);
//! * `sleep` — `sleep(50)` lasts at least 50 ticks; `sleep(6001)` is refused;
//! * `orphan` — spawn an orphan and exit at once;
//! * `orphan-child` — (spawned by `orphan`) sleep 20 ticks, report, exit 0;
//! * `svc-report` — `svc_report` (needs the Service capability) is refused
//!   for a pid that is not our child, for a name the kernel reserves, for a
//!   row someone else owns, and for `ready` (we are not init); a report about
//!   our own child is accepted;
//! * `svc-basic` — the same without the owned-row case (the selftest has no
//!   `tickd` row);
//! * `svc-denied` — without the Service capability `svc_report` is refused
//!   at the capability gate.
//!
//! Every check prints a `PROCPROBE-…` marker; a failure prints
//! `PROCPROBE-FAILED step=<step>` and exits 1.

#![no_std]
#![no_main]

use ulib::{
    args, exit, sleep_ticks, spawn, spawn_args, split_args, svc_report, uptime_ticks, wait,
    wait_nohang, write, write_u64, yield_now, CAP_SPAWN, ERR_AGAIN, ERR_INVAL, ERR_NOENT, ERR_PERM,
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

/// `wait(0)` blocks until ANY child ends (PROC1-001, found by the syscall
/// fuzzer: the wake-up matched the pid alone, so this slept forever). The
/// child cannot have ended when the wait starts - it has not run yet, and it
/// yields three times before it exits - so this is the blocking path.
fn wait_any() {
    let c = spawn("/bin/child");
    if is_err(c) {
        fail("wait-any-spawn");
    }
    let st = wait(0);
    write("PROCPROBE-WAIT-ANY status=");
    write_u64(st);
    write("\n");
    if st != CHILD_STATUS {
        fail("wait-any-status");
    }
    // Delivered means collected: no child is left.
    let mut status = 0u64;
    if wait_nohang(0, &mut status) != ERR_NOENT {
        fail("wait-any-collected");
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

/// The 32-byte `svc_report` record (`kernel_core::svcreport`), built by hand
/// so the probe checks the ABI independently of the kernel's own codec.
fn record(event: u8, restarts: u32, pid: u64, name: &[u8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[0] = event;
    r[4..8].copy_from_slice(&restarts.to_le_bytes());
    r[8..16].copy_from_slice(&pid.to_le_bytes());
    r[16..16 + name.len()].copy_from_slice(name);
    r
}

const EV_START: u8 = 1;
const EV_DONE: u8 = 3;
const EV_READY: u8 = 5;

fn expect(got: u64, want: u64, step: &str, marker: &str) {
    if got != want {
        fail(step);
    }
    write(marker);
}

fn svc(owned_row: bool) {
    let c = spawn("/bin/child");
    if is_err(c) {
        fail("svc-spawn");
    }
    // pid 1 is not our child (whatever it is, if anything).
    expect(
        svc_report(&record(EV_START, 0, 1, b"fake")),
        ERR_NOENT,
        "not_child",
        "PROCPROBE-SVCREPORT-NOT-CHILD-OK\n",
    );
    expect(
        svc_report(&record(EV_START, 0, c, b"echod")),
        ERR_INVAL,
        "reserved_name",
        "PROCPROBE-SVCREPORT-RESERVED-OK\n",
    );
    if owned_row {
        expect(
            svc_report(&record(EV_START, 0, c, b"tickd")),
            ERR_PERM,
            "not_owner",
            "PROCPROBE-SVCREPORT-NOT-OWNER-OK\n",
        );
    }
    expect(
        svc_report(&record(EV_READY, 0, 0, b"probe")),
        ERR_PERM,
        "not_init",
        "PROCPROBE-SVCREPORT-READY-REFUSED-OK\n",
    );
    if svc_report(&record(EV_START, 0, c, b"probe-svc")) != 0 {
        fail("svc-start");
    }
    if wait(c) != CHILD_STATUS {
        fail("svc-wait");
    }
    expect(
        svc_report(&record(EV_DONE, 0, c, b"probe-svc")),
        0,
        "svc-done",
        "PROCPROBE-SVCREPORT-ACCEPTED-OK\n",
    );
    write("PROCPROBE-SVCREPORT-OK\n");
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
            wait_any();
            sleep_check();
            orphan();
        }
        Some(b"foreign") => match it.next().and_then(parse_u64) {
            Some(pid) => foreign(pid),
            None => fail("foreign-usage"),
        },
        Some(b"nohang") => nohang(),
        Some(b"wait-any") => wait_any(),
        Some(b"sleep") => sleep_check(),
        Some(b"orphan") => orphan(),
        Some(b"gui-foreign") => match it.next().and_then(parse_u64) {
            // A window this process does not own: no present, no events.
            Some(win) => {
                if ulib::gui_present(win) != ERR_PERM {
                    fail("gui-present-foreign");
                }
                let mut rec = [0u8; 8];
                expect(
                    ulib::gui_event(win, &mut rec),
                    ERR_PERM,
                    "gui-event-foreign",
                    "PROCPROBE-GUI-FOREIGN-REFUSED\n",
                )
            }
            None => fail("gui-foreign-usage"),
        },
        Some(b"gui-quota") => {
            // The per-process window quota: 4, then ERR_AGAIN.
            let mut made = 0u64;
            for _ in 0..6 {
                let w = ulib::gui_create(16, 16, 20, 40, "q");
                if is_err(w) {
                    if w != ERR_AGAIN {
                        fail("gui-quota-err");
                    }
                    break;
                }
                made += 1;
            }
            if made != 4 {
                fail("gui-quota-count");
            }
            write("PROCPROBE-GUI-QUOTA-OK windows=4\n");
        }
        Some(b"gui-pixels") => {
            // V1.0 (GUI1-001): a Ring 3 owner's windows are bounded by a
            // pixel quota (half the compositor budget), not just a window
            // count, so one program cannot take the whole budget. Two
            // 1024x512 windows (524288 px each) reach the quota exactly; the
            // third is refused with ERR_AGAIN, before the 4-window cap.
            let mut made = 0u64;
            for _ in 0..4 {
                let w = ulib::gui_create(1024, 512, 20, 40, "px");
                if is_err(w) {
                    if w != ERR_AGAIN {
                        fail("gui-pixels-err");
                    }
                    break;
                }
                made += 1;
            }
            if made != 2 {
                fail("gui-pixels-count");
            }
            write("PROCPROBE-GUI-PIXELS-OK windows=2\n");
        }
        Some(b"console-read") => {
            let mut line = [0u8; 16];
            expect(
                ulib::console_read(&mut line),
                ERR_PERM,
                "console-read",
                "PROCPROBE-CONSOLE-READ-REFUSED\n",
            )
        }
        Some(b"svc-report") => svc(true),
        Some(b"svc-basic") => svc(false),
        Some(b"svc-denied") => expect(
            svc_report(&record(EV_START, 0, 1, b"x")),
            ERR_PERM,
            "svc-denied",
            "PROCPROBE-SVCREPORT-DENIED-OK\n",
        ),
        Some(b"orphan-child") => {
            if sleep_ticks(20) != 0 {
                fail("orphan-sleep");
            }
            write("PROCPROBE-ORPHAN-CHILD-DONE\n");
            exit(0)
        }
        _ => {
            write("PROCPROBE-USAGE all|foreign <pid>|nohang|wait-any|sleep|orphan|svc-report|svc-basic|svc-denied\n");
            exit(2)
        }
    }
    write("PROCPROBE-OK\n");
    exit(0)
}
