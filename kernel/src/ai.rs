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
                Ok(_) => "ok",
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
