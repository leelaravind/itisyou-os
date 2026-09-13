//! `/sbin/init` — the userspace init (V0.10, INIT10-001/002).
//!
//! Reads `/etc/init.conf` with `fs_read` (from the initramfs), starts every
//! service in dependency order with exactly the capabilities the file names
//! (delegated from init's own, so never more than init holds), supervises
//! them with `wait_nohang` and `sleep` under the bounded restart policy, and
//! reports every lifecycle event to the kernel's service table
//! (`svc_report`). The grammar and the policy are the host-tested
//! `kernel_core::initconf` and `kernel_core::supervise`.
//!
//! The first argument picks the mode:
//!
//! * `check <path>` — parse only: `INIT-CONFIG-OK path=… services=… order=…`
//!   (exit 0) or `INIT-CONFIG-ERROR path=… line=… reason=…` (exit 2);
//! * `once <path>` — start and supervise until every service has ended, then
//!   `INIT-ONCE-DONE services=…` (exit 0; 3 if any service failed);
//! * no argument — pid-1 mode: `/etc/init.conf`, report `ready` once every
//!   service has been started, then supervise forever, collecting any orphan
//!   the kernel hands over.
//!
//! No allocator; every buffer is on the stack.

#![no_std]
#![no_main]

use kernel_core::initconf::{self, Config, Service, MAX_FILE, MAX_SERVICES};
use kernel_core::procstatus::{self, Status};
use kernel_core::progargs::MAX_BLOCK;
use kernel_core::service::ServiceState;
use kernel_core::supervise;
use kernel_core::svcreport::{self, Event, Report, ServiceName};
use ulib::{
    args, exit, fs_read, sleep_ticks, spawn_args, spawn_caps, split_args, svc_report, wait_nohang,
    write, write_u64, ERR_2BIG, ERR_AGAIN, ERR_NOENT, ERR_PERM,
};

const DEFAULT_CONFIG: &str = "/etc/init.conf";
/// Ticks between two looks while every service is running.
const POLL_TICKS: u64 = 5;
/// Ticks between two looks when init has no children at all.
const IDLE_TICKS: u64 = 50;

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

/// Print an error return as the kernel names it (`ERR_*` is `u64::MAX - k`).
fn write_err(v: u64) {
    write_u64(u64::MAX - v);
}

fn write_bytes(b: &[u8]) {
    if let Ok(s) = core::str::from_utf8(b) {
        write(s);
    }
}

fn write_order(cfg: &Config, order: &[usize]) {
    for (i, &idx) in order.iter().enumerate() {
        if i > 0 {
            write(",");
        }
        if let Some(s) = cfg.get(idx) {
            write(s.name);
        }
    }
}

/// Read and parse a config. On failure: the line (0 = the whole file) and
/// the stable reason name.
fn load<'a>(path: &str, buf: &'a mut [u8; MAX_FILE]) -> Result<Config<'a>, (usize, &'static str)> {
    let n = fs_read(path, buf);
    if n == ERR_PERM {
        return Err((0, "read_denied"));
    }
    if n == ERR_NOENT {
        return Err((0, "not_found"));
    }
    if n == ERR_2BIG {
        return Err((0, "too_large"));
    }
    if is_err(n) || n as usize > MAX_FILE {
        return Err((0, "read_failed"));
    }
    let buf: &'a [u8; MAX_FILE] = buf;
    initconf::parse_bytes(&buf[..n as usize]).map_err(|e| (e.line, e.reason.name()))
}

fn config_error(path: &str, line: usize, reason: &str) {
    write("INIT-CONFIG-ERROR path=");
    write(path);
    write(" line=");
    write_u64(line as u64);
    write(" reason=");
    write(reason);
    write("\n");
}

/// Start one service with exactly its configured capabilities. `spawn_caps`
/// when it has no arguments (so the kernel audits a plain `spawn`),
/// `spawn_args` otherwise.
fn start(svc: &Service) -> u64 {
    let mut block = [0u8; MAX_BLOCK];
    match svc.encode_args(&mut block) {
        Ok(0) => spawn_caps(svc.path, svc.caps),
        Ok(n) => spawn_args(svc.path, svc.caps, &block[..n]),
        // Never happens for a config `parse` accepted.
        Err(_) => ERR_2BIG,
    }
}

/// Report one event to the kernel's service table; a refusal is printed and
/// the service keeps running (the table is a view, not the supervisor).
fn report(event: Event, name: &str, long_running: bool, clean: bool, restarts: u32, pid: u64) {
    let Ok(name_field) = ServiceName::new(name) else {
        return;
    };
    let rec = svcreport::encode(&Report {
        event,
        long_running,
        clean_exit: clean,
        restarts,
        pid,
        name: name_field,
    });
    let r = svc_report(&rec);
    if r != 0 {
        write("INIT-REPORT-REFUSED name=");
        write(name);
        write(" err=");
        write_err(r);
        write("\n");
    }
}

/// Supervision state of one service.
#[derive(Clone, Copy)]
struct Slot {
    pid: u64,
    restarts: u32,
    ended: bool,
    failed: bool,
}

const IDLE_SLOT: Slot = Slot {
    pid: 0,
    restarts: 0,
    ended: false,
    failed: false,
};

fn spawn_error(name: &str, err: u64) {
    write("INIT-SPAWN-ERROR name=");
    write(name);
    write(" err=");
    write_err(err);
    write("\n");
}

fn status_name(word: u64) {
    match procstatus::decode(word) {
        Some(Status::Exited(code)) => {
            write("exit:");
            write_u64(u64::from(code));
        }
        Some(Status::Faulted(v)) => {
            write("fault:");
            write_u64(u64::from(v));
        }
        Some(Status::Killed) => {
            write("killed");
        }
        None => {
            write("unknown");
        }
    }
}

/// Start everything, then supervise. `once`: return when every service has
/// ended; otherwise never return.
fn run(path: &str, once: bool) -> ! {
    let mut buf = [0u8; MAX_FILE];
    let cfg = match load(path, &mut buf) {
        Ok(c) => c,
        Err((line, reason)) => {
            config_error(path, line, reason);
            if once {
                exit(2)
            }
            // pid 1 with no usable config: say so, report ready with no
            // services, and stay (an init that exits is restarted, and would
            // fail the same way).
            report(Event::Ready, "init", false, false, 0, 0);
            loop {
                collect_orphans();
                sleep_ticks(IDLE_TICKS);
            }
        }
    };
    let (order, n) = match cfg.order() {
        Ok(o) => o,
        Err(e) => {
            config_error(path, e.line, e.reason.name());
            exit(2)
        }
    };
    write("INIT-CONFIG path=");
    write(path);
    write(" services=");
    write_u64(n as u64);
    write(" order=");
    write_order(&cfg, &order[..n]);
    write("\n");

    let mut slots = [IDLE_SLOT; MAX_SERVICES];
    for &idx in &order[..n] {
        let Some(svc) = cfg.get(idx) else { continue };
        let pid = start(svc);
        if is_err(pid) {
            spawn_error(svc.name, pid);
            slots[idx].ended = true;
            slots[idx].failed = true;
            report(Event::Failed, svc.name, svc.long_running(), false, 0, 0);
            continue;
        }
        slots[idx].pid = pid;
        write("INIT-START name=");
        write(svc.name);
        write(" pid=");
        write_u64(pid);
        write("\n");
        report(Event::Start, svc.name, svc.long_running(), false, 0, pid);
    }
    if !once {
        report(Event::Ready, "init", false, false, 0, 0);
    }

    loop {
        let mut status = 0u64;
        let r = wait_nohang(0, &mut status);
        if r == ERR_AGAIN {
            sleep_ticks(POLL_TICKS);
            continue;
        }
        if r == ERR_NOENT || is_err(r) {
            // No children at all.
            if once {
                break;
            }
            sleep_ticks(IDLE_TICKS);
            continue;
        }
        let pid = r;
        match (0..cfg.len).find(|&i| slots[i].pid == pid && !slots[i].ended) {
            Some(idx) => {
                let Some(svc) = cfg.get(idx) else { continue };
                ended(svc, &mut slots[idx], status);
            }
            None => {
                write("INIT-REAPED-ORPHAN pid=");
                write_u64(pid);
                write(" status=");
                status_name(status);
                write("\n");
            }
        }
        if once && slots[..cfg.len].iter().all(|s| s.ended) {
            break;
        }
    }
    let failed = slots[..cfg.len].iter().any(|s| s.failed);
    write("INIT-ONCE-DONE services=");
    write_u64(cfg.len as u64);
    write("\n");
    exit(if failed { 3 } else { 0 })
}

/// With no config there is nothing to supervise, but orphans still arrive.
fn collect_orphans() {
    let mut status = 0u64;
    loop {
        let r = wait_nohang(0, &mut status);
        if is_err(r) {
            return;
        }
        write("INIT-REAPED-ORPHAN pid=");
        write_u64(r);
        write(" status=");
        status_name(status);
        write("\n");
    }
}

/// One of our services ended with `status`: apply the restart policy.
fn ended(svc: &Service, slot: &mut Slot, status: u64) {
    let clean = status == 0;
    match supervise::decide(svc.restart, clean, slot.restarts) {
        ServiceState::Restarting { restarts } => {
            slot.restarts = restarts;
            let pid = start(svc);
            if is_err(pid) {
                spawn_error(svc.name, pid);
                slot.ended = true;
                slot.failed = true;
                report(
                    Event::Failed,
                    svc.name,
                    svc.long_running(),
                    clean,
                    restarts,
                    0,
                );
                return;
            }
            slot.pid = pid;
            write("INIT-RESTART name=");
            write(svc.name);
            write(" pid=");
            write_u64(pid);
            write(" restarts=");
            write_u64(u64::from(restarts));
            write(" clean_exit=");
            write(if clean { "true" } else { "false" });
            write("\n");
            report(
                Event::Restart,
                svc.name,
                svc.long_running(),
                clean,
                restarts,
                pid,
            );
        }
        ServiceState::Done => {
            slot.ended = true;
            write("INIT-DONE name=");
            write(svc.name);
            write("\n");
            report(
                Event::Done,
                svc.name,
                svc.long_running(),
                clean,
                slot.restarts,
                0,
            );
        }
        ServiceState::Failed { restarts } => {
            slot.ended = true;
            slot.failed = true;
            write("INIT-FAILED name=");
            write(svc.name);
            write(" restarts=");
            write_u64(u64::from(restarts));
            write("\n");
            report(
                Event::Failed,
                svc.name,
                svc.long_running(),
                clean,
                restarts,
                0,
            );
        }
        ServiceState::Stopped | ServiceState::Running => {}
    }
}

fn check(path: &str) -> ! {
    let mut buf = [0u8; MAX_FILE];
    let cfg = match load(path, &mut buf) {
        Ok(c) => c,
        Err((line, reason)) => {
            config_error(path, line, reason);
            exit(2)
        }
    };
    match cfg.order() {
        Ok((order, n)) => {
            write("INIT-CONFIG-OK path=");
            write(path);
            write(" services=");
            write_u64(n as u64);
            write(" order=");
            write_order(&cfg, &order[..n]);
            write("\n");
            exit(0)
        }
        Err(e) => {
            config_error(path, e.line, e.reason.name());
            exit(2)
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; MAX_BLOCK];
    let n = args(&mut block);
    let block: &[u8] = if n as usize <= MAX_BLOCK {
        &block[..n as usize]
    } else {
        &[]
    };
    let mut it = split_args(block);
    let mode = it.next();
    let path = it.next().and_then(|p| core::str::from_utf8(p).ok());
    match (mode, path) {
        (None, _) => run(DEFAULT_CONFIG, false),
        (Some(b"check"), Some(p)) => check(p),
        (Some(b"once"), Some(p)) => run(p, true),
        (Some(m), _) => {
            write("INIT-USAGE mode=");
            write_bytes(m);
            write(" (check <path> | once <path> | no argument)\n");
            exit(2)
        }
    }
}
