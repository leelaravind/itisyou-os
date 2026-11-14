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
use kernel_core::caps::{cap_ipc_channel, CAP_IPC, CAP_SPAWN};

/// IPC on channels 0-1 only, the echo service's (V1.0, ADR-0025: an IPC
/// capability names its channels).
const ECHO_IPC: u64 = CAP_IPC | cap_ipc_channel(0) | cap_ipc_channel(1);
use kernel_core::service::{self, on_exit, startup_order, ServiceState};
use kernel_core::svcreport::ServiceName;

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
        caps: ECHO_IPC,
        long_running: false,
    },
    ServiceDef {
        name: "client",
        path: "/bin/svc-client",
        deps: &["echod"],
        caps: ECHO_IPC,
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

/// One bounded always-on slice (V0.10), then init's own supervision: a dead
/// `/sbin/init` is restarted from here, outside any lock. (The V0.8 kernel
/// supervised a static background set here; since V0.10 `/sbin/init` does,
/// from `/etc/init.conf`.)
pub fn pump_slice() {
    proc::run_slice(
        kernel_core::cosched::SLICE_TICKS,
        kernel_core::cosched::SLICE_QUANTA,
        kernel_core::cosched::SLICE_MAX_MS,
    );
    crate::initd::supervise();
}

/// Let init start its services before the console starts (V0.10): slices
/// until init reports `ready` or `max_ticks` pass. Settling first makes the
/// boot deterministic — init is pid 1 and its services come next, in the
/// config's order — even though the test harness types every command the
/// moment the console is up.
pub fn settle(max_ticks: u64) -> bool {
    let start = crate::interrupts::ticks();
    while !crate::initd::ready() && crate::interrupts::ticks() - start < max_ticks {
        pump_slice();
    }
    let ready = crate::initd::ready();
    crate::serial_println!(
        "[ITISYOU:INIT] settled ready={ready} ticks={}",
        crate::interrupts::ticks() - start
    );
    ready
}

/// Live status of one supervised service (queryable via `svc`).
#[derive(Debug, Clone, Copy)]
pub struct Status {
    /// Validated (`[a-z0-9-]{1,15}`, V0.10): rows can come from a Ring 3
    /// supervisor now, not only from the kernel's static tables.
    pub name: ServiceName,
    pub state: ServiceState,
    pub pid: u64,
    pub restarts: u32,
    /// Who reports this service: 0 = the kernel, otherwise the supervisor's
    /// pid (V0.10, `svc_report`).
    pub owner: u64,
}

static STATUS: Mutex<Vec<Status>> = Mutex::new(Vec::new());

/// Most rows the service table holds (V0.10: supervisors can add rows).
const MAX_ROWS: usize = 16;

/// A kernel-owned row (the static REGISTRY and BACKGROUND names are all valid).
fn set_state(name: &'static str, state: ServiceState, pid: u64, restarts: u32) {
    let name = ServiceName::new(name).expect("static service names are valid");
    set_state_owned(name, state, pid, restarts, 0);
}

fn set_state_owned(name: ServiceName, state: ServiceState, pid: u64, restarts: u32, owner: u64) {
    let mut table = STATUS.lock();
    match table.iter_mut().find(|s| s.name == name) {
        Some(s) => {
            s.state = state;
            s.pid = pid;
            s.restarts = restarts;
            s.owner = owner;
        }
        None => table.push(Status {
            name,
            state,
            pid,
            restarts,
            owner,
        }),
    }
    let name = name.as_str();
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
    // Reset only the on-demand services' rows. The persistent services
    // (init's, since V0.10) keep running across an `svc` run, so clearing the
    // whole table would erase live state and make them invisible to the
    // operator for the rest of the session.
    STATUS
        .lock()
        .retain(|s| !REGISTRY.iter().any(|d| d.name == s.name.as_str()));
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
                    .find(|s| s.name.as_str() == def.name)
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
                Some(proc::ProcState::Faulted { .. }) | Some(proc::ProcState::Killed) => {
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
            .filter(|s| REGISTRY.iter().any(|d| d.name == s.name.as_str()))
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

// --- svc_report (V0.10, SVC10-001) -------------------------------------------

/// Refuse a supervisor report: say why, and audit it.
fn refuse_report(caller: u64, reason: &str, err: u64) -> u64 {
    crate::serial_println!(
        "[ITISYOU:SVC] svc_report refused pid={caller} init={} reason={reason}",
        crate::initd::is_init(caller)
    );
    crate::audit::denied("svc_report", kernel_core::caps::CAP_SERVICE);
    err
}

/// svc_report(req_ptr, len) — a Ring 3 supervisor reports one service event
/// (V0.10, syscall 39; the Service capability with ADMIN was checked by the
/// dispatcher). The record (`kernel_core::svcreport`, exactly 32 bytes) is
/// decoded strictly; then the kernel checks what it can know itself:
///
/// * `ready` — only init may say its start phase is over (`not_init`);
/// * a name the kernel's own on-demand registry uses is never a
///   supervisor's (`reserved_name`);
/// * `start`/`restart` — the pid must be the reporter's own uncollected
///   child (`not_child`), and the caps printed are the kernel's record of
///   that child, not the reporter's claim;
/// * a row belongs to whoever created it, unless that supervisor is dead
///   (`not_owner`; the kernel's rows are never taken over);
/// * at most 16 rows (`table_full`).
///
/// The name and the policy (`long_running`) remain the supervisor's claim,
/// and every accepted line names the supervisor (`supervisor=<pid>`).
pub fn sys_svc_report(req_ptr: u64, len: u64) -> u64 {
    use crate::syscall::{ERR_AGAIN, ERR_INVAL, ERR_NOENT, ERR_PERM};
    use kernel_core::svcreport::{self, Event, REPORT_LEN};
    let caller = crate::syscall::CURRENT_PID.load(core::sync::atomic::Ordering::SeqCst);
    if len != REPORT_LEN as u64 {
        return refuse_report(caller, "bad_length", ERR_INVAL);
    }
    let bytes = match crate::syscall::copy_from_user(req_ptr, len, len) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let report = match svcreport::decode(&bytes) {
        Ok(r) => r,
        Err(e) => return refuse_report(caller, e.name(), ERR_INVAL),
    };
    if report.event == Event::Ready {
        if !crate::initd::is_init(caller) {
            return refuse_report(caller, "not_init", ERR_PERM);
        }
        crate::initd::mark_ready();
        let owned = STATUS.lock().iter().filter(|s| s.owner == caller).count();
        crate::serial_println!("[ITISYOU:INIT] ready pid={caller} services={owned}");
        return 0;
    }
    let name = report.name;
    if REGISTRY.iter().any(|d| d.name == name.as_str()) {
        return refuse_report(caller, "reserved_name", ERR_INVAL);
    }
    let caps = if matches!(report.event, Event::Start | Event::Restart) {
        match crate::proc::child_caps(caller, report.pid) {
            Some(c) => Some(c),
            None => return refuse_report(caller, "not_child", ERR_NOENT),
        }
    } else {
        None
    };
    {
        let table = STATUS.lock();
        match table.iter().find(|s| s.name == name) {
            Some(row)
                if row.owner != caller && (row.owner == 0 || !crate::proc::is_dead(row.owner)) =>
            {
                drop(table);
                return refuse_report(caller, "not_owner", ERR_PERM);
            }
            None if table.len() >= MAX_ROWS => {
                drop(table);
                return refuse_report(caller, "table_full", ERR_AGAIN);
            }
            _ => {}
        }
    }
    let (n, pid, k) = (name.as_str(), report.pid, report.restarts);
    let state = match report.event {
        Event::Start => {
            crate::serial_println!(
                "[ITISYOU:SVC] bg_start name={n} pid={pid} caps={:#x} long_running={} supervisor={caller}",
                caps.unwrap_or(0),
                report.long_running
            );
            ServiceState::Running
        }
        Event::Restart => {
            crate::serial_println!(
                "[ITISYOU:SVC] bg_restart name={n} pid={pid} restarts={k} clean_exit={} supervisor={caller}",
                report.clean_exit
            );
            ServiceState::Running
        }
        Event::Done => {
            crate::serial_println!(
                "[ITISYOU:SVC] bg_done name={n} restarts={k} supervisor={caller}"
            );
            ServiceState::Done
        }
        _ => {
            crate::serial_println!(
                "[ITISYOU:SVC] bg_failed name={n} restarts={k} supervisor={caller}"
            );
            ServiceState::Failed { restarts: k }
        }
    };
    let row_pid = if state == ServiceState::Running {
        pid
    } else {
        0
    };
    set_state_owned(name, state, row_pid, k, caller);
    // V0.11: a retried service under verification fails on a restart,
    // failure or completion its supervisor reports (ACT11-002).
    if crate::initd::is_init(caller) {
        crate::ai::on_service_event(n, report.event);
    }
    0
}
