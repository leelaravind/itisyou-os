//! The AI-native system layer's kernel side (V0.11, ADR-0024).
//!
//! The kernel runs no model and makes no decision from one. It holds the
//! provenance anchor — the SHA-256 of the model the build trained, compiled
//! in — and, as the steps of ADR-0024 land, the approved view (`sys_view`),
//! the proposal table and the console's approval path. Inference itself runs
//! in Ring 3 (`/bin/inferd`).

/// SHA-256 of `/etc/ai/diag.model` as the build trained it (`kernel/build.rs`
/// checks it against the pin `ai/diag.model.sha256`).
pub const MODEL_SHA256: [u8; 32] = *include_bytes!(concat!(env!("OUT_DIR"), "/model_sha256.bin"));

/// Where the model lives in the initramfs.
pub const MODEL_PATH: &str = "/etc/ai/diag.model";

fn hex<'a>(d: &[u8; 32], out: &'a mut [u8; 64]) -> &'a str {
    kernel_core::audit_chain::format_head(d, out)
}

// --- The approved view (VIEW11-001) -----------------------------------------

use kernel_core::sysview::{self, ServiceRow, SvcState, View};

fn svc_state(s: kernel_core::service::ServiceState) -> SvcState {
    use kernel_core::service::ServiceState as S;
    match s {
        S::Stopped => SvcState::Stopped,
        S::Running => SvcState::Running,
        S::Done => SvcState::Done,
        S::Restarting { .. } => SvcState::Restarting,
        S::Failed { .. } => SvcState::Failed,
    }
}

/// Fill the view from the kernel's own tables — nothing a program reported
/// that the kernel has not checked, and nothing beyond the allow-listed
/// counts and rows (`kernel_core::sysview`).
pub fn build_view() -> View {
    let (processes, runnable, waiting, ended) = crate::proc::state_counts();
    let (max_gap_ms, busy, idle, job) = crate::sched::view_counters();
    let (audit_total, audit_denials) = crate::audit::counts();
    let (ring_records, ring_denials) = crate::audit::ring_counts();
    let net = crate::net::stats();
    let mut v = View {
        uptime_ticks: crate::interrupts::ticks(),
        processes: sysview::sat16(processes as u64),
        runnable: sysview::sat16(runnable as u64),
        waiting: sysview::sat16(waiting as u64),
        ended: sysview::sat16(ended as u64),
        always_on: crate::sched::enabled(),
        paused: crate::sched::enabled() && crate::sched::paused(),
        truncated: false,
        max_gap_ms: sysview::sat32(max_gap_ms),
        busy_slices: sysview::sat32(busy),
        idle_slices: sysview::sat32(idle),
        job_slices: sysview::sat32(job),
        audit_total: sysview::sat32(audit_total),
        audit_denials: sysview::sat32(audit_denials),
        recent_records: sysview::sat16(ring_records as u64),
        recent_denials: sysview::sat16(ring_denials as u64),
        net_rx: sysview::sat32(net.rx_frames),
        net_tx: sysview::sat32(net.tx_frames),
        net_refused: sysview::sat32(net.rx_malformed),
        ..View::default()
    };
    crate::services::with_status(|rows| {
        for s in rows {
            // Names are validated when a row is made (V0.10), so a row the
            // view cannot carry is a kernel bug; skip it rather than guess.
            if let Some(row) = ServiceRow::new(
                s.name.as_str(),
                svc_state(s.state),
                s.restarts,
                s.owner != 0 && crate::initd::is_init(s.owner),
            ) {
                v.push_service(row);
            }
        }
    });
    v
}

/// A view served to a process: when, and the exact bytes.
#[derive(Clone, Copy)]
struct Served {
    pid: u64,
    tick: u64,
    bytes: [u8; sysview::LEN],
}

/// The last view served to each of the most recent processes that asked.
/// Pids are never reused, and a process's entry is dropped when it ends.
const SERVED_SLOTS: usize = 16;
static SERVED: crate::sync::Mutex<[Option<Served>; SERVED_SLOTS]> =
    crate::sync::Mutex::new([None; SERVED_SLOTS]);

/// Remember `bytes` as the view just served to `pid`.
pub fn served(pid: u64, bytes: &[u8; sysview::LEN]) {
    let tick = crate::interrupts::ticks();
    {
        let mut table = SERVED.lock();
        let slot = match table.iter().position(|s| s.is_some_and(|s| s.pid == pid)) {
            Some(i) => i,
            None => match table.iter().position(Option::is_none) {
                Some(i) => i,
                // Full: the oldest entry goes.
                None => table
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, s)| s.map_or(0, |s| s.tick))
                    .map_or(0, |(i, _)| i),
            },
        };
        table[slot] = Some(Served {
            pid,
            tick,
            bytes: *bytes,
        });
    }
    let digest = kernel_core::sha256::digest(bytes);
    let mut buf = [0u8; 64];
    let hex = hex(&digest, &mut buf);
    crate::serial_println!(
        "[ITISYOU:AI] view_served pid={pid} bytes={} sha256={} tick={tick}",
        bytes.len(),
        &hex[..16]
    );
}

/// The last view served to `pid` and its tick.
pub fn last_served(pid: u64) -> Option<(u64, [u8; sysview::LEN])> {
    SERVED
        .lock()
        .iter()
        .flatten()
        .find(|s| s.pid == pid)
        .map(|s| (s.tick, s.bytes))
}

/// `pid` ended: forget its view.
pub fn forget(pid: u64) {
    for slot in SERVED.lock().iter_mut() {
        if slot.is_some_and(|s| s.pid == pid) {
            *slot = None;
        }
    }
}

/// Boot check: the model in the initramfs is the one the kernel was built
/// with, and it decodes. Printed, never fatal — without a model the agent
/// cannot diagnose, but nothing else depends on it.
pub fn boot_check() {
    let mut buf = [0u8; 64];
    let compiled = hex(&MODEL_SHA256, &mut buf);
    match crate::fs::read(MODEL_PATH) {
        Ok(bytes) => {
            let found = kernel_core::sha256::digest(bytes);
            let decoded = match kernel_core::model::decode(bytes) {
                Ok(m) => {
                    // Kept only to CHECK claims: the kernel recomputes a
                    // proposal's condition with it and acts on nothing it says.
                    if found == MODEL_SHA256 {
                        *MODEL.lock() = Some(m);
                    }
                    "ok"
                }
                Err(e) => e.name(),
            };
            crate::serial_println!(
                "[ITISYOU:AI] model sha256={compiled} bytes={} initramfs_match={} decode={decoded}",
                bytes.len(),
                found == MODEL_SHA256
            );
        }
        Err(_) => crate::serial_println!(
            "[ITISYOU:AI] model sha256={compiled} initramfs_match=false reason=missing"
        ),
    }
}

// --- Proposals (ACT11-001) ---------------------------------------------------
//
// An agent holding `propose` files a record; the kernel checks everything it
// can know itself and keeps what passes in a small table. NOTHING here acts:
// the only path from a proposal to an action is the console's `approve`.

use kernel_core::policy::{self, Action, Entry, Event, Facts, Record, Refusal, ServiceFacts};

/// The shipped model, decoded at boot, used only to recompute the condition
/// a proposal cites.
static MODEL: crate::sync::Mutex<Option<kernel_core::model::Model>> = crate::sync::Mutex::new(None);
static PROPOSALS: crate::sync::Mutex<policy::Table> = crate::sync::Mutex::new(policy::Table::new());

/// Whether `/etc/init.conf` (the initramfs copy init itself reads) defines a
/// service called `name`.
fn in_init_conf(name: &str) -> bool {
    crate::fs::read("/etc/init.conf")
        .ok()
        .and_then(|b| core::str::from_utf8(b).ok())
        .and_then(|t| kernel_core::initconf::parse(t).ok())
        .is_some_and(|c| c.services().any(|s| s.name == name))
}

/// What the kernel knows now that decides whether `record` applies.
fn facts(record: &Record) -> Facts {
    let target = if record.action.has_target() {
        let name = record.target_str();
        crate::services::with_status(|rows| {
            rows.iter().find(|r| r.name.as_str() == name).map(|r| {
                (
                    matches!(r.state, kernel_core::service::ServiceState::Failed { .. }),
                    r.owner != 0 && crate::initd::is_init(r.owner),
                )
            })
        })
        .map(|(failed, owned_by_init)| ServiceFacts {
            failed,
            owned_by_init,
            in_init_conf: in_init_conf(name),
        })
    } else {
        None
    };
    Facts {
        always_on: crate::sched::enabled(),
        paused: crate::sched::paused(),
        target,
    }
}

/// The checks, in order; the first that fails names the refusal.
fn check(pid: u64, bytes: &[u8]) -> Result<(Record, u64), Refusal> {
    let record = policy::decode(bytes)?;
    if record.model != MODEL_SHA256 {
        return Err(Refusal::UnknownModel);
    }
    let (tick, view_bytes) = last_served(pid).ok_or(Refusal::SnapshotMismatch)?;
    if kernel_core::sha256::digest(&view_bytes) != record.view {
        return Err(Refusal::SnapshotMismatch);
    }
    let now = crate::interrupts::ticks();
    if now.saturating_sub(tick) > policy::VIEW_MAX_AGE_TICKS {
        return Err(Refusal::SnapshotStale);
    }
    let view = View::decode(&view_bytes).map_err(|_| Refusal::SnapshotMismatch)?;
    let fired = MODEL
        .lock()
        .as_ref()
        .map(|m| m.detect(&sysview::features(&view)))
        .ok_or(Refusal::UnknownModel)?;
    if !fired.has(record.condition) {
        return Err(Refusal::DiagnosisMismatch);
    }
    if !policy::applies(record.action, &facts(&record)) {
        return Err(Refusal::NotApplicable);
    }
    let id = PROPOSALS.lock().insert(pid, record, now)?;
    Ok((record, id))
}

fn target_or_dash(r: &Record) -> &str {
    if r.action.has_target() {
        r.target_str()
    } else {
        "-"
    }
}

/// propose(record, len) (V0.11, syscall 43; SystemAdministration USE): file
/// a proposal. Returns its id, `ERR_INVAL` for a malformed record,
/// `ERR_AGAIN` for a full table or a proposal already pending, `ERR_PERM`
/// for everything the kernel refused on its own knowledge. Every refusal is
/// printed and audited with its reason.
pub fn sys_propose(ptr: u64, len: u64) -> u64 {
    use crate::syscall::{ERR_AGAIN, ERR_INVAL, ERR_PERM};
    let pid = crate::syscall::CURRENT_PID.load(core::sync::atomic::Ordering::SeqCst);
    let result = if len != policy::RECORD_LEN as u64 {
        Err(Refusal::BadLength)
    } else {
        match crate::syscall::copy_from_user(ptr, len, policy::RECORD_LEN as u64) {
            Ok(bytes) => check(pid, &bytes),
            Err(e) => return e,
        }
    };
    match result {
        Ok((r, id)) => {
            let mut m = [0u8; 64];
            let mut v = [0u8; 64];
            let (model, view) = (hex(&r.model, &mut m), hex(&r.view, &mut v));
            crate::serial_println!(
                "[ITISYOU:AI] proposal_submitted id={id} pid={pid} action={} target={} condition={} model_check=ok snapshot_check=ok diagnosis_check=ok applies=ok",
                r.action.name(),
                target_or_dash(&r),
                r.condition.name()
            );
            crate::audit::allowed(
                "proposal_submitted",
                kernel_core::caps::CAP_PROPOSE,
                Some(alloc::format!(
                    "id={id} action={} target={} condition={} model={} view={}",
                    r.action.name(),
                    target_or_dash(&r),
                    r.condition.name(),
                    &model[..16],
                    &view[..16]
                )),
            );
            id
        }
        Err(why) => {
            crate::serial_println!(
                "[ITISYOU:AI] proposal_refused pid={pid} reason={}",
                why.name()
            );
            crate::audit::denied_reason("propose", kernel_core::caps::CAP_PROPOSE, why.name());
            match why {
                Refusal::AlreadyPending | Refusal::TableFull => ERR_AGAIN,
                Refusal::BadLength
                | Refusal::BadVersion
                | Refusal::UnknownAction
                | Refusal::UnknownCondition
                | Refusal::UnknownRunbook
                | Refusal::BadField
                | Refusal::BadReserved => ERR_INVAL,
                _ => ERR_PERM,
            }
        }
    }
}

/// Expire pending proposals past their TTL, printing and auditing each.
fn expire_now() {
    let now = crate::interrupts::ticks();
    PROPOSALS.lock().expire(now, |e| {
        crate::serial_println!("[ITISYOU:AI] proposal_expired id={} reason=ttl", e.id);
        crate::audit::allowed(
            "proposal_expired",
            0,
            Some(alloc::format!("id={} reason=ttl", e.id)),
        );
    });
}

/// What the operator is shown before deciding: built only from fields the
/// kernel validated.
fn preview(e: &Entry) {
    let r = &e.record;
    match r.action {
        Action::ResumeScheduler => crate::serial_println!(
            "[ITISYOU:AI] preview id={} turn background scheduling back on; verify {} ticks: background processes progress at busy points; rollback: {}",
            e.id,
            r.action.verify_ticks(),
            r.action.rollback()
        ),
        Action::RetryService => crate::serial_println!(
            "[ITISYOU:AI] preview id={} ask /sbin/init to start {} once more with its restart count reset; verify {} ticks: no failure or restart of {}; rollback: {}",
            e.id,
            r.target_str(),
            r.action.verify_ticks(),
            r.target_str(),
            r.action.rollback()
        ),
    }
}

/// Console `proposals`: every entry, oldest first, with its preview.
pub fn console_list() {
    expire_now();
    let now = crate::interrupts::ticks();
    let entries: alloc::vec::Vec<Entry> = PROPOSALS.lock().entries().collect();
    if entries.is_empty() {
        crate::serial_println!("proposals: none");
    }
    for e in &entries {
        crate::serial_println!(
            "[ITISYOU:AI] proposal id={} state={} action={} target={} condition={} risk={} reversible={} submitter={} age_ticks={}",
            e.id,
            e.state.name(),
            e.record.action.name(),
            target_or_dash(&e.record),
            e.record.condition.name(),
            e.record.action.risk(),
            e.record.action.reversible(),
            e.submitter,
            now.saturating_sub(e.filed)
        );
        preview(e);
    }
}

/// Console `deny <id>`: final; a denied proposal can never be approved.
pub fn console_deny(id: u64) {
    expire_now();
    let result = PROPOSALS.lock().transition(id, Event::Deny);
    match result {
        Ok(_) => {
            crate::serial_println!("[ITISYOU:AI] proposal_denied id={id} by=console");
            crate::audit::allowed(
                "proposal_denied",
                0,
                Some(alloc::format!("id={id} by=console")),
            );
        }
        Err(e) => crate::serial_println!("deny: id={id} refused reason={}", e.name()),
    }
}

/// Console `approve <id>` (S9): the only path from a proposal to an action.
///
/// Re-checks everything that can have changed since the proposal was filed -
/// its TTL and whether the action still applies - then executes the action
/// in kernel code, verifies its post-condition over the action's window, and
/// rolls back if the check fails. Every step is printed and audited with the
/// proposal's provenance.
pub fn console_approve(id: u64) {
    expire_now();
    let Some(entry) = PROPOSALS.lock().get(id) else {
        crate::serial_println!("approve: id={id} refused reason=no_such_proposal");
        return;
    };
    if entry.state != policy::State::Pending {
        crate::serial_println!("approve: id={id} refused reason=already_decided");
        return;
    }
    let r = entry.record;
    // The system may have moved on since the proposal was filed.
    if !policy::applies(r.action, &facts(&r)) {
        let _ = PROPOSALS.lock().transition(id, Event::Expire);
        crate::serial_println!("approve: id={id} refused reason=precondition_changed");
        crate::serial_println!("[ITISYOU:AI] proposal_expired id={id} reason=precondition_changed");
        crate::audit::allowed(
            "proposal_expired",
            0,
            Some(alloc::format!("id={id} reason=precondition_changed")),
        );
        return;
    }
    if r.action == Action::RetryService && !RETRY_AVAILABLE {
        crate::serial_println!("approve: id={id} refused reason=action_unavailable");
        return;
    }
    if PROPOSALS.lock().transition(id, Event::Approve).is_err() {
        crate::serial_println!("approve: id={id} refused reason=already_decided");
        return;
    }
    crate::serial_println!("[ITISYOU:AI] proposal_approved id={id} by=console");
    crate::audit::allowed(
        "proposal_approved",
        0,
        Some(alloc::format!(
            "id={id} action={} by=console",
            r.action.name()
        )),
    );
    let _ = PROPOSALS.lock().transition(id, Event::Execute);
    let mut m = [0u8; 64];
    let mut v = [0u8; 64];
    let (model, view) = (hex(&r.model, &mut m), hex(&r.view, &mut v));
    let detail = alloc::format!(
        "id={id} action={} target={} condition={} approved_by=console model={} view={}",
        r.action.name(),
        target_or_dash(&r),
        r.condition.name(),
        &model[..16],
        &view[..16]
    );
    let passed = match r.action {
        Action::ResumeScheduler => execute_resume(id, &detail),
        Action::RetryService => false,
    };
    let event = if passed { Event::Pass } else { Event::Fail };
    let _ = PROPOSALS.lock().transition(id, event);
}

/// Whether retry-service can execute yet (S10 lands the init mailbox).
const RETRY_AVAILABLE: bool = false;

/// resume-scheduler: turn background slices back on, then require that
/// other processes actually progress at busy points during the window -
/// the same measurement as `busy`, so a resume that did nothing fails and is
/// rolled back.
fn execute_resume(id: u64, detail: &str) -> bool {
    crate::sched::resume();
    crate::serial_println!(
        "[ITISYOU:AI] action_executed id={id} action=resume-scheduler approved_by=console"
    );
    crate::audit::allowed(
        "action_executed",
        0,
        Some(alloc::string::String::from(detail)),
    );
    let window_ms = policy::Action::ResumeScheduler.verify_ticks() * 10;
    let before = crate::sched::other_quanta_total();
    let end = crate::interrupts::tsc() + crate::interrupts::cycles_for_ms(window_ms);
    while crate::interrupts::tsc() < end {
        crate::sched::safe_point();
        core::hint::spin_loop();
    }
    let others = crate::sched::other_quanta_total() - before;
    let passed = others > 0 && !crate::sched::paused();
    crate::serial_println!(
        "[ITISYOU:AI] action_verified id={id} result={} window_ms={window_ms} other_quanta={others}",
        if passed { "pass" } else { "fail" }
    );
    crate::audit::allowed(
        "action_verified",
        0,
        Some(alloc::format!(
            "id={id} result={} other_quanta={others}",
            if passed { "pass" } else { "fail" }
        )),
    );
    if !passed {
        crate::sched::pause();
        crate::serial_println!(
            "[ITISYOU:AI] action_rolled_back id={id} action=resume-scheduler undo=paused-again"
        );
        crate::audit::allowed(
            "action_rolled_back",
            0,
            Some(alloc::format!("id={id} undo=paused-again")),
        );
    }
    passed
}
