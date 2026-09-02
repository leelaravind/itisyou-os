//! System service supervisor (V0.7, ADR-0012).
//!
//! Services are ordinary Ring 3 processes — no hidden privileged daemons.
//! A static registry declares each service's binary, dependencies, and the
//! EXACT capabilities it runs with (least privilege). The supervisor starts
//! them in deterministic dependency order (cycle-checked by host-tested
//! `kernel_core::service`), co-schedules them preemptively, contains their
//! crashes, applies the bounded restart policy, and emits one structured
//! `[ITISYOU:SVC]` marker per state transition — the machine-readable
//! service diagnostics the tests assert on.

use crate::{proc, user};
use alloc::vec::Vec;
use kernel_core::caps::{CAP_IPC, CAP_SPAWN};
use kernel_core::service::{self, on_exit, startup_order, ServiceState};
use spin::Mutex;

/// A declared system service.
pub struct ServiceDef {
    pub name: &'static str,
    pub path: &'static str,
    pub deps: &'static [&'static str],
    /// Exactly the capabilities this service runs with — nothing implicit.
    pub caps: u64,
}

/// The service registry. `echod` serves IPC; `client` (a oneshot task that
/// depends on it) proves the service actually serves; `crashd` is the
/// deliberately faulting service exercising containment + bounded restart.
pub static REGISTRY: &[ServiceDef] = &[
    ServiceDef {
        name: "echod",
        path: "/bin/echo-svc",
        deps: &[],
        caps: CAP_IPC,
    },
    ServiceDef {
        name: "client",
        path: "/bin/svc-client",
        deps: &["echod"],
        caps: CAP_IPC,
    },
    ServiceDef {
        name: "crashd",
        path: "/bin/crashy-svc",
        deps: &["echod"],
        caps: 0,
    },
];

/// Live status of one supervised service (queryable via `svc`).
#[derive(Debug, Clone, Copy)]
pub struct Status {
    pub name: &'static str,
    pub state: ServiceState,
    pub pid: u64,
    pub restarts: u32,
}

static STATUS: Mutex<Vec<Status>> = Mutex::new(Vec::new());

fn set_state(name: &'static str, state: ServiceState, pid: u64, restarts: u32) {
    let mut table = STATUS.lock();
    match table.iter_mut().find(|s| s.name == name) {
        Some(s) => {
            s.state = state;
            s.pid = pid;
            s.restarts = restarts;
        }
        None => table.push(Status {
            name,
            state,
            pid,
            restarts,
        }),
    }
    let label = match state {
        ServiceState::Stopped => "stopped",
        ServiceState::Running => "running",
        ServiceState::Done => "done",
        ServiceState::Restarting { .. } => "restarting",
        ServiceState::Failed { .. } => "failed",
    };
    crate::serial_println!("[ITISYOU:SVC] name={name} state={label} pid={pid} restarts={restarts}");
}

/// Run `f` over a snapshot of the service table.
pub fn with_status<R>(f: impl FnOnce(&[Status]) -> R) -> R {
    let table = STATUS.lock();
    let snap: Vec<Status> = table.clone();
    drop(table);
    f(&snap)
}

/// Outcome of one supervised run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    pub started: usize,
    pub done: usize,
    pub failed: usize,
    pub restarts_performed: u32,
}

/// Start every registered service in dependency order and supervise until
/// all reach a terminal state (Done/Failed), applying the bounded restart
/// policy to abnormal exits. `max_ticks` bounds each scheduling slice so a
/// misbehaving service cannot hang the supervisor.
pub fn run_supervised(max_ticks: u64) -> Result<RunReport, service::OrderError> {
    STATUS.lock().clear();
    // Deterministic, cycle-checked startup order (host-tested logic).
    let mut decl: Vec<(&str, &[&str])> = Vec::new();
    for def in REGISTRY {
        decl.push((def.name, def.deps));
    }
    let (order, n) = startup_order(&decl)?;

    // pid + restart count per registry index.
    let mut pids = [0u64; service::MAX_SERVICES];
    let mut restarts = [0u32; service::MAX_SERVICES];

    for (slot, &idx) in order[..n].iter().enumerate() {
        let def = &REGISTRY[idx];
        match user::load_with(def.path, def.caps, None) {
            Ok(p) => {
                let pid = proc::admit(p);
                pids[idx] = pid;
                crate::serial_println!(
                    "[ITISYOU:SVC] start name={} pid={pid} order={slot} caps={:#x}",
                    def.name,
                    def.caps,
                );
                set_state(def.name, ServiceState::Running, pid, 0);
            }
            Err(_) => {
                set_state(def.name, ServiceState::Failed { restarts: 0 }, 0, 0);
            }
        }
    }

    // Supervision passes: schedule, inspect, restart-or-finalize. Bounded.
    let mut report = RunReport {
        started: n,
        done: 0,
        failed: 0,
        restarts_performed: 0,
    };
    for _pass in 0..16 {
        proc::run_until_pid_idle_bounded(max_ticks);
        let mut any_restart = false;
        let mut all_terminal = true;
        for (idx, def) in REGISTRY.iter().enumerate() {
            let current = with_status(|t| {
                t.iter()
                    .find(|s| s.name == def.name)
                    .map(|s| s.state)
                    .unwrap_or(ServiceState::Stopped)
            });
            if matches!(current, ServiceState::Done | ServiceState::Failed { .. }) {
                continue;
            }
            match proc::state_of(pids[idx]) {
                Some(proc::ProcState::Exited(code)) => {
                    proc::reap(pids[idx]);
                    let next = on_exit(code == 0, restarts[idx]);
                    match next {
                        ServiceState::Done => {
                            set_state(def.name, next, pids[idx], restarts[idx]);
                        }
                        ServiceState::Restarting { restarts: r } => {
                            restarts[idx] = r;
                            any_restart = true;
                            restart(def, idx, &mut pids, r);
                        }
                        ServiceState::Failed { .. } => {
                            set_state(def.name, next, pids[idx], restarts[idx]);
                        }
                        _ => {}
                    }
                }
                Some(proc::ProcState::Faulted { .. }) => {
                    proc::reap(pids[idx]);
                    let next = on_exit(false, restarts[idx]);
                    match next {
                        ServiceState::Restarting { restarts: r } => {
                            restarts[idx] = r;
                            any_restart = true;
                            restart(def, idx, &mut pids, r);
                        }
                        other => set_state(def.name, other, pids[idx], restarts[idx]),
                    }
                }
                Some(_) => all_terminal = false, // still runnable/blocked
                None => {
                    // Slot vanished without a recorded terminal state.
                    set_state(
                        def.name,
                        ServiceState::Failed {
                            restarts: restarts[idx],
                        },
                        0,
                        restarts[idx],
                    );
                }
            }
        }
        if !any_restart && all_terminal {
            break;
        }
    }

    with_status(|t| {
        for s in t {
            match s.state {
                ServiceState::Done => report.done += 1,
                ServiceState::Failed { restarts } => {
                    report.failed += 1;
                    report.restarts_performed += restarts;
                }
                _ => {}
            }
        }
    });
    crate::audit::allowed("svc_run", CAP_SPAWN, None);
    crate::serial_println!(
        "[ITISYOU:SVC] supervisor done started={} done={} failed={} restarts={}",
        report.started,
        report.done,
        report.failed,
        report.restarts_performed,
    );
    Ok(report)
}

fn restart(def: &ServiceDef, idx: usize, pids: &mut [u64], count: u32) {
    match user::load_with(def.path, def.caps, None) {
        Ok(p) => {
            let pid = proc::admit(p);
            pids[idx] = pid;
            set_state(
                def.name,
                ServiceState::Restarting { restarts: count },
                pid,
                count,
            );
        }
        Err(_) => set_state(def.name, ServiceState::Failed { restarts: count }, 0, count),
    }
}
