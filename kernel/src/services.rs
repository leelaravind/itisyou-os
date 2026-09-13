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

use crate::sync::Mutex;
use crate::{proc, user};
use alloc::vec::Vec;
use kernel_core::caps::{CAP_IPC, CAP_SPAWN};
use kernel_core::service::{self, on_exit, startup_order, ServiceState};

/// A declared system service.
pub struct ServiceDef {
    pub name: &'static str,
    pub path: &'static str,
    pub deps: &'static [&'static str],
    /// Exactly the capabilities this service runs with — nothing implicit.
    pub caps: u64,
    /// A service that is expected to run for the life of the system rather
    /// than complete. Its clean exit is a FAULT, not success: a daemon that
    /// returns has stopped doing its job, so the supervisor restarts it under
    /// the same bounded policy as a crash.
    pub long_running: bool,
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
        long_running: false,
    },
    ServiceDef {
        name: "client",
        path: "/bin/svc-client",
        deps: &["echod"],
        caps: CAP_IPC,
        long_running: false,
    },
    ServiceDef {
        name: "crashd",
        path: "/bin/crashy-svc",
        deps: &["echod"],
        caps: 0,
        long_running: false,
    },
];

/// Services started at boot and supervised for the life of the system (V0.8),
/// as opposed to [`REGISTRY`], whose members are run to completion on demand.
///
/// These are ordinary Ring 3 processes holding only the capabilities named
/// here — there is still no privileged daemon. They are co-scheduled with the
/// interactive shell (see [`pump`]), so the system does real background work
/// while remaining responsive.
pub static BACKGROUND: &[ServiceDef] = &[
    ServiceDef {
        name: "tickd",
        path: "/bin/tickd",
        deps: &[],
        // IPC only: tickd answers status requests and does nothing else. It
        // cannot spawn, touch the filesystem, or reach any device.
        caps: CAP_IPC,
        long_running: true,
    },
    ServiceDef {
        name: "flapd",
        path: "/bin/flapd",
        deps: &[],
        // Zero authority: a service that only proves the failure path needs
        // none, and giving it any would weaken the test.
        caps: 0,
        long_running: true,
    },
];

/// Bounded restart policy for background services. A daemon that dies is
/// restarted, but a crash loop must terminate rather than burn the machine —
/// after this many restarts the service is marked Failed and left alone.
///
/// Aliased to the host-tested [`service::RESTART_LIMIT`] rather than restated:
/// two independently written ceilings would eventually disagree, and the
/// on-demand and background supervisors must apply the same policy.
pub const BACKGROUND_MAX_RESTARTS: u32 = service::RESTART_LIMIT;

/// Live supervision state for the background services, parallel to
/// [`BACKGROUND`].
struct Background {
    pids: [u64; MAX_BACKGROUND],
    restarts: [u32; MAX_BACKGROUND],
    /// Slot is no longer supervised: the service reached a terminal state
    /// (Failed at the restart ceiling, or Done for a non-daemon).
    retired: [bool; MAX_BACKGROUND],
    started: bool,
}

const MAX_BACKGROUND: usize = 8;

static BG: Mutex<Background> = Mutex::new(Background {
    pids: [0; MAX_BACKGROUND],
    restarts: [0; MAX_BACKGROUND],
    retired: [false; MAX_BACKGROUND],
    started: false,
});

/// Start the persistent services. Called once during boot, before the shell
/// takes over the console.
pub fn start_background() {
    let mut bg = BG.lock();
    if bg.started {
        return;
    }
    bg.started = true;
    for (idx, def) in BACKGROUND.iter().enumerate().take(MAX_BACKGROUND) {
        match user::load_with(def.path, def.caps, None) {
            Ok(p) => {
                let pid = proc::admit(p);
                bg.pids[idx] = pid;
                crate::serial_println!(
                    "[ITISYOU:SVC] bg_start name={} pid={pid} caps={:#x} long_running={}",
                    def.name,
                    def.caps,
                    def.long_running,
                );
                set_state(def.name, ServiceState::Running, pid, 0);
            }
            Err(_) => {
                bg.retired[idx] = true;
                set_state(def.name, ServiceState::Failed { restarts: 0 }, 0, 0);
                crate::serial_println!("[ITISYOU:SVC] bg_start name={} result=failed", def.name);
            }
        }
    }
}

/// Give the background services a bounded slice of CPU, then apply the
/// restart policy to any that died.
///
/// Called from the shell's idle path, which previously just span waiting for
/// the next serial byte. That spin is the natural place for background work:
/// the shell stays responsive because the slice is bounded in real time, and
/// services make progress whenever the operator is not typing.
pub fn pump(max_ticks: u64) {
    if !BG.lock().started {
        return;
    }
    proc::run_until_pid_idle_bounded(max_ticks);
    supervise_background();
}

/// One bounded always-on slice (V0.10), then the restart policy. Unlike
/// [`pump`] the slice runs whether or not the V0.8 background set was
/// started: whatever is runnable gets the CPU.
pub fn pump_slice() {
    proc::run_slice(
        kernel_core::cosched::SLICE_TICKS,
        kernel_core::cosched::SLICE_QUANTA,
        kernel_core::cosched::SLICE_MAX_MS,
    );
    if BG.lock().started {
        supervise_background();
    }
}

/// The V0.8 background restart policy (moved verbatim out of [`pump`]).
fn supervise_background() {
    let mut bg = BG.lock();
    for (idx, def) in BACKGROUND.iter().enumerate().take(MAX_BACKGROUND) {
        if bg.retired[idx] {
            continue;
        }
        let pid = bg.pids[idx];
        let died = match proc::state_of(pid) {
            Some(proc::ProcState::Exited(code)) => Some(code == 0),
            Some(proc::ProcState::Faulted { .. }) => Some(false),
            // Still runnable/blocked, or the slot is gone (already reaped).
            Some(_) => None,
            None => Some(false),
        };
        let Some(clean) = died else {
            continue;
        };
        proc::reap(pid);
        // For a long-running service even a clean exit counts as a failure:
        // it was supposed to still be running.
        let treat_as_success = clean && !def.long_running;
        let next = on_exit(treat_as_success, bg.restarts[idx]);
        match next {
            ServiceState::Restarting { restarts } if restarts <= BACKGROUND_MAX_RESTARTS => {
                bg.restarts[idx] = restarts;
                match user::load_with(def.path, def.caps, None) {
                    Ok(p) => {
                        let new_pid = proc::admit(p);
                        bg.pids[idx] = new_pid;
                        crate::serial_println!(
                            "[ITISYOU:SVC] bg_restart name={} pid={new_pid} restarts={restarts} clean_exit={clean}",
                            def.name,
                        );
                        set_state(def.name, ServiceState::Running, new_pid, restarts);
                    }
                    Err(_) => {
                        bg.retired[idx] = true;
                        set_state(def.name, ServiceState::Failed { restarts }, 0, restarts);
                    }
                }
            }
            // A background service that is NOT long-running finished the work it
            // was started for. Stop supervising it, but say `done` — calling a
            // completed task a failure would make the diagnostics lie.
            ServiceState::Done => {
                bg.retired[idx] = true;
                let restarts = bg.restarts[idx];
                crate::serial_println!(
                    "[ITISYOU:SVC] bg_done name={} restarts={restarts}",
                    def.name,
                );
                set_state(def.name, ServiceState::Done, 0, restarts);
            }
            other => {
                bg.retired[idx] = true;
                let restarts = bg.restarts[idx];
                crate::serial_println!(
                    "[ITISYOU:SVC] bg_failed name={} restarts={restarts}",
                    def.name,
                );
                set_state(def.name, other, 0, restarts);
            }
        }
    }
}

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
    // Reset only the on-demand services' rows. The persistent [`BACKGROUND`]
    // services keep running across an `svc` run, so clearing the whole table
    // would erase live state and make the supervised daemons invisible to the
    // operator for the rest of the session.
    STATUS
        .lock()
        .retain(|s| !REGISTRY.iter().any(|d| d.name == s.name));
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

    // Count only the on-demand services: the status table also carries the
    // persistent background daemons, and a report about THIS supervised run
    // must not fold in a background service's state (a Failed flapd would
    // otherwise show up as `started=3 failed=2`, which is arithmetic nonsense).
    with_status(|t| {
        for s in t
            .iter()
            .filter(|s| REGISTRY.iter().any(|d| d.name == s.name))
        {
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
