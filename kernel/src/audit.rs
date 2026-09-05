//! Audit / provenance trail (V0.7).
//!
//! Every privileged platform action — and every DENIED capability check —
//! is recorded: sequence number, timer tick, acting pid, action name, the
//! capability involved, and the result. Records land in a bounded in-memory
//! ring (queryable via the `audit` shell command) and are mirrored as
//! machine-readable `[ITISYOU:AUDIT]` serial markers so automated tests can
//! assert on them. Contents are metadata only — no user data/payloads are
//! logged. Future AI-agent actions must flow through the same syscall →
//! capability check → service path, so they inherit this provenance trail
//! automatically (intelligence ≠ authority; see ADR-0012).

use alloc::collections::VecDeque;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// One audit record.
#[derive(Debug, Clone)]
pub struct Record {
    pub seq: u64,
    pub tick: u64,
    pub pid: u64,
    pub action: &'static str,
    /// Capability bit involved (0 = not capability-related).
    pub cap: u64,
    pub ok: bool,
    /// Short free-form detail (a path/app name), never payload contents.
    pub detail: Option<String>,
}

const RING_MAX: usize = 64;

static RING: Mutex<VecDeque<Record>> = Mutex::new(VecDeque::new());
static SEQ: AtomicU64 = AtomicU64::new(0);
static DENIALS: AtomicU64 = AtomicU64::new(0);

fn push(action: &'static str, cap: u64, ok: bool, detail: Option<String>) {
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    let tick = crate::interrupts::ticks();
    let result = if ok { "ok" } else { "denied" };
    match &detail {
        Some(d) => crate::serial_println!(
            "[ITISYOU:AUDIT] seq={seq} tick={tick} pid={pid} action={action} cap={cap:#x} result={result} detail=\"{d}\""
        ),
        None => crate::serial_println!(
            "[ITISYOU:AUDIT] seq={seq} tick={tick} pid={pid} action={action} cap={cap:#x} result={result}"
        ),
    }
    if !ok {
        DENIALS.fetch_add(1, Ordering::SeqCst);
    }
    let mut ring = RING.lock();
    if ring.len() >= RING_MAX {
        ring.pop_front();
    }
    ring.push_back(Record {
        seq,
        tick,
        pid,
        action,
        cap,
        ok,
        detail,
    });
}

/// Record a denied capability check.
pub fn denied(action: &'static str, cap: u64) {
    push(action, cap, false, None);
}

/// Record a denied V0.8 handle check with the resource class and the exact
/// reason (`revoked`, `expired`, `scope_denied`, `not_owner`, …). A bare
/// "permission denied" is not diagnosable; the reason distinguishes an
/// authority that was never held from one that was withdrawn or timed out.
pub fn denied_capability(
    action: &'static str,
    kind: kernel_core::capability::CapabilityKind,
    reason: &'static str,
) {
    push(
        action,
        0,
        false,
        Some(alloc::format!("kind={} reason={reason}", kind.name())),
    );
}

/// Record a permitted privileged action (with optional short detail).
pub fn allowed(action: &'static str, cap: u64, detail: Option<String>) {
    push(action, cap, true, detail);
}

/// Total records ever written / total denials (diagnostic evidence).
pub fn counts() -> (u64, u64) {
    (SEQ.load(Ordering::SeqCst), DENIALS.load(Ordering::SeqCst))
}

/// Run `f` over a snapshot of the retained ring (newest last).
pub fn with_records<R>(f: impl FnOnce(&[Record]) -> R) -> R {
    let ring = RING.lock();
    let snapshot: alloc::vec::Vec<Record> = ring.iter().cloned().collect();
    drop(ring);
    f(&snapshot)
}
