//! `agent` - the diagnostic agent (V0.11, AGENT11-001, ADR-0024).
//!
//! A deterministic Ring 3 program: it reads the approved view (`sys_view`),
//! derives the features, asks `/bin/inferd` which conditions hold, looks up
//! each condition's runbook entry in `/etc/ai/kb/`, and prints its diagnosis
//! and the action the runbook names. It does not plan, generate text or
//! learn, and it holds no authority to act: it is started with the view, IPC
//! and read access under `/etc/ai`, nothing else - and for "resume the
//! scheduler" or "retry a service" no syscall exists at all.
//!
//! `run /bin/agent sys_view,ipc,fs_read /etc/ai -- diagnose`
//! `run /bin/agent sys_view,propose,ipc,fs_read /etc/ai -- propose` (S8) -
//! diagnose, then file ONE proposal (the kernel keeps one pending per
//! submitter) for the first actionable condition, a paused scheduler first:
//! `AGENT-PROPOSAL filed id= action= target=`. Filing is all it can do; only
//! the console's `approve` turns a proposal into an action.
//!
//! Lines (all prefixed `AGENT-`): `VIEW` (the view's digest prefix and a few
//! counts), `DIAGNOSIS conditions=<set> model=<prefix> scores=<a,b,c>`, and
//! per condition `RUNBOOK id= sha= title=` and `ACTION id= action=`.
//! Failures: `VIEW-DENIED`, `INFER-TIMEOUT`, `RUNBOOK-MISSING id=`.

#![no_std]
#![no_main]

use kernel_core::infer::{self, Reply, Request};
use kernel_core::scenario::Condition;
use kernel_core::sysview::{self, SvcState, View};
use ulib::{
    args, exit, fs_read, getpid, msg_recv, msg_send, sleep_ticks, split_args, sys_view,
    uptime_ticks, write, write_hex8, write_u64, ERR_PERM,
};

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

struct Out;

impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write(s);
        Ok(())
    }
}

fn write_prefix(d: &[u8; 32]) {
    for b in &d[..8] {
        write_hex8(*b);
    }
}

fn write_i64(v: i64) {
    if v < 0 {
        write("-");
    }
    write_u64(v.unsigned_abs());
}

fn fail(line: &str) -> ! {
    write(line);
    write("\n");
    exit(1)
}

/// The view, its bytes and its digest.
fn view() -> (View, [u8; 32]) {
    let mut buf = [0u8; sysview::LEN];
    let n = sys_view(&mut buf);
    if n == ERR_PERM {
        fail("AGENT-VIEW-DENIED err=perm");
    }
    if is_err(n) {
        fail("AGENT-VIEW-FAILED");
    }
    match View::decode(&buf) {
        Ok(v) => (v, kernel_core::sha256::digest(&buf)),
        Err(e) => {
            write("AGENT-VIEW-DECODE-FAILED reason=");
            fail(e.name())
        }
    }
}

/// Ask inferd, with a nonce and a bounded wait (3 s).
fn ask(x: [i32; sysview::FEATURES]) -> Reply {
    let nonce = (uptime_ticks() << 20) ^ getpid();
    let req = Request { nonce, x };
    if msg_send(infer::REQUEST_CHANNEL, &req.encode()) != infer::REQUEST_LEN as u64 {
        fail("AGENT-INFER-SEND-FAILED");
    }
    let mut buf = [0u8; 256];
    for _ in 0..300 {
        let n = msg_recv(infer::REPLY_CHANNEL, &mut buf);
        if is_err(n) {
            sleep_ticks(1);
            continue;
        }
        if let Ok(r) = Reply::decode(&buf[..n as usize]) {
            if r.nonce == nonce {
                return r;
            }
        }
        // Not ours (another client's, or garbage on an unauthenticated
        // channel): discard and keep waiting.
    }
    fail("AGENT-INFER-TIMEOUT")
}

/// The runbook path for a condition (a closed table: nothing else is read).
fn runbook_path(c: Condition) -> &'static str {
    match c {
        Condition::ServiceFailed => "/etc/ai/kb/service_failed.txt",
        Condition::SchedulerPaused => "/etc/ai/kb/scheduler_paused.txt",
        Condition::DenialBurst => "/etc/ai/kb/denial_burst.txt",
    }
}

fn runbook(c: Condition, v: &View) {
    let mut buf = [0u8; 2048];
    let n = fs_read(runbook_path(c), &mut buf);
    if is_err(n) {
        write("AGENT-RUNBOOK-MISSING id=");
        write(c.name());
        write("\n");
        return;
    }
    let text = &buf[..n as usize];
    let digest = kernel_core::sha256::digest(text);
    let title = text.split(|&b| b == b'\n').next().unwrap_or(b"");
    write("AGENT-RUNBOOK id=");
    write(c.name());
    write(" sha=");
    write_prefix(&digest);
    write(" title=");
    write(core::str::from_utf8(title).unwrap_or("?"));
    write("\n");
    // The action the runbook names; S8 files it as a proposal.
    write("AGENT-ACTION id=");
    write(c.name());
    write(" action=");
    match c {
        Condition::ServiceFailed => {
            write("retry-service target=");
            let failed = v.rows().iter().find(|r| r.state == SvcState::Failed);
            write(failed.map_or("?", |r| r.name_str()));
        }
        Condition::SchedulerPaused => {
            write("resume-scheduler");
        }
        Condition::DenialBurst => {
            write("none");
        }
    }
    write("\n");
}

/// Diagnose, and return what the proposal step needs.
fn diagnose() -> (View, [u8; 32], Reply) {
    let (v, view_digest) = view();
    write("AGENT-VIEW sha=");
    write_prefix(&view_digest);
    write(" processes=");
    write_u64(u64::from(v.processes));
    write(" services=");
    write_u64(u64::from(v.service_count));
    write("\n");
    let reply = ask(sysview::features(&v));
    write("AGENT-DIAGNOSIS conditions=");
    let _ = reply.conditions.write(&mut Out);
    write(" model=");
    write_prefix(&reply.model);
    write(" scores=");
    for (i, s) in reply.scores.iter().enumerate() {
        if i > 0 {
            write(",");
        }
        write_i64(*s);
    }
    write(" \n");
    for c in Condition::ALL {
        if reply.conditions.has(c) {
            runbook(c, &v);
        }
    }
    (v, view_digest, reply)
}

/// File ONE proposal - the kernel keeps at most one pending per submitter -
/// for the first actionable condition, a paused scheduler first (nothing
/// else recovers while it is paused).
fn propose() {
    use kernel_core::policy::{allowed_action, Record, NAME_LEN};
    let (v, view_digest, reply) = diagnose();
    let mut filed = false;
    for c in [
        Condition::SchedulerPaused,
        Condition::ServiceFailed,
        Condition::DenialBurst,
    ] {
        if !reply.conditions.has(c) {
            continue;
        }
        let Some(action) = allowed_action(c) else {
            write("AGENT-PROPOSAL-NONE id=");
            write(c.name());
            write(" reason=no_action\n");
            continue;
        };
        if filed {
            write("AGENT-PROPOSAL-SKIPPED id=");
            write(c.name());
            write(" reason=one_pending_per_agent\n");
            continue;
        }
        let mut target = [0u8; NAME_LEN];
        if action.has_target() {
            let Some(row) = v.rows().iter().find(|r| r.state == SvcState::Failed) else {
                continue;
            };
            target.copy_from_slice(&row.name);
        }
        let record = Record {
            action,
            condition: c,
            target,
            model: reply.model,
            view: view_digest,
        };
        let r = ulib::propose(&record.encode());
        if is_err(r) {
            write("AGENT-PROPOSAL-REFUSED action=");
            write(action.name());
            write(" err=");
            write_u64(u64::MAX - r);
            write("\n");
        } else {
            write("AGENT-PROPOSAL filed id=");
            write_u64(r);
            write(" action=");
            write(action.name());
            write(" target=");
            write(if action.has_target() {
                record.target_str()
            } else {
                "-"
            });
            write("\n");
            filed = true;
        }
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
        Some(b"diagnose") => {
            diagnose();
        }
        Some(b"propose") => propose(),
        _ => fail("AGENT-USAGE agent diagnose|propose"),
    }
    exit(0)
}
