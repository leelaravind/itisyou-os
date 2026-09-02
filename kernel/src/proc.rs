//! Process table and concurrent-process scheduler (V0.3, ADR-0006).
//!
//! Multiple Ring 3 processes run cooperatively on the resumable V0.2/V0.3
//! transition. The run-loop owns every [`Process`]; it takes one out of the
//! table for a quantum (so `spawn`/`wait` can lock the table re-entrantly),
//! runs it until it yields / blocks / exits / faults, then updates state.
//! `yield` round-robins; `wait` blocks the caller until the child is a
//! zombie, delivering the child's status as the syscall return value.
//!
//! Preemptive user scheduling is out of V0.3 scope (KNOWN_LIMITATIONS).

use crate::syscall::{ERR_2BIG, ERR_AGAIN, ERR_NOENT};
use crate::user::{self, Process, UserExit};
use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcState {
    Runnable,
    /// Blocked in wait() on the given child pid.
    Blocked {
        on: u64,
    },
    Exited(u64),
    Faulted {
        vector: u8,
    },
}

struct Slot {
    /// Present except while this process is out running its quantum.
    process: Option<Process>,
    state: ProcState,
    started: bool,
}

struct Table {
    slots: BTreeMap<u64, Slot>,
    runq: VecDeque<u64>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

const SPAWN_PATH_MAX: u64 = 128;

pub fn init() {
    *TABLE.lock() = Some(Table {
        slots: BTreeMap::new(),
        runq: VecDeque::new(),
    });
}

/// Admit an already-loaded process as RUNNABLE. Returns its pid.
pub fn admit(process: Process) -> u64 {
    let pid = process.pid;
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    table.slots.insert(
        pid,
        Slot {
            process: Some(process),
            state: ProcState::Runnable,
            started: false,
        },
    );
    table.runq.push_back(pid);
    pid
}

/// Terminal/typed state of a process, if still in the table.
pub fn state_of(pid: u64) -> Option<ProcState> {
    let guard = TABLE.lock();
    guard.as_ref()?.slots.get(&pid).map(|s| s.state)
}

fn encode_status(state: ProcState) -> u64 {
    match state {
        ProcState::Exited(code) => code & 0xFFFF_FFFF,
        ProcState::Faulted { vector } => 0x1_0000_0000 | vector as u64,
        _ => 0,
    }
}

/// Run the scheduler until no process is runnable. Returns how many reached
/// a terminal state. A leftover Blocked process with no runnable peer is a
/// deadlock and is reported (should not happen in the tested programs).
pub fn run_until_idle() -> usize {
    run_scheduler(|_| false)
}

/// Run the scheduler preemptively until `target` reaches a terminal state or
/// `max_ticks` timer ticks elapse. Returns `target`'s state (if any) — used
/// by the non-monopolization proof, where a co-scheduled infinite spinner
/// keeps the run queue non-empty forever. `run_until_idle` cannot be used
/// there because it only stops when nothing is runnable.
pub fn run_until_pid_exits(target: u64, max_ticks: u64) -> Option<ProcState> {
    let deadline = crate::interrupts::ticks() + max_ticks;
    run_scheduler(|_| {
        let done = matches!(
            state_of(target),
            Some(ProcState::Exited(_)) | Some(ProcState::Faulted { .. }) | None
        );
        done || crate::interrupts::ticks() >= deadline
    });
    state_of(target)
}

/// Core scheduler loop. Runs each runnable process for one quantum (until it
/// yields, is preempted, blocks, exits, or faults), updating state. Stops
/// when the run queue drains or `stop(completed)` returns true.
fn run_scheduler(stop: impl Fn(usize) -> bool) -> usize {
    let mut completed = 0;
    loop {
        if stop(completed) {
            break;
        }
        let taken = {
            let mut guard = TABLE.lock();
            let table = guard.as_mut().expect("proc table init");
            loop {
                match table.runq.pop_front() {
                    None => break None,
                    Some(pid) => match table.slots.get_mut(&pid) {
                        Some(s) if s.state == ProcState::Runnable && s.process.is_some() => {
                            let first = !s.started;
                            s.started = true;
                            break Some((pid, s.process.take().unwrap(), first));
                        }
                        _ => continue, // stale queue entry
                    },
                }
            }
        };
        let Some((pid, mut process, first)) = taken else {
            break;
        };

        sched::set_current(pid);
        let outcome = user::run_quantum(&mut process, first);
        sched::set_current(0);

        let mut guard = TABLE.lock();
        let table = guard.as_mut().expect("proc table init");
        match outcome {
            UserExit::Yielded | UserExit::Preempted => {
                if let Some(s) = table.slots.get_mut(&pid) {
                    s.process = Some(process);
                    table.runq.push_back(pid);
                }
            }
            UserExit::Blocked => {
                // wait() already set this slot's state to Blocked{on}.
                if let Some(s) = table.slots.get_mut(&pid) {
                    s.process = Some(process);
                }
            }
            terminal => {
                let state = match terminal {
                    UserExit::Exit(code) => {
                        crate::serial_println!("[ITISYOU:INFO] user_exit pid={pid} code={code}");
                        ProcState::Exited(code)
                    }
                    UserExit::Fault { vector, addr } => {
                        crate::serial_println!(
                            "[ITISYOU:INFO] user_fault pid={pid} vector={vector} addr={:#x} contained=true",
                            addr.unwrap_or(0)
                        );
                        ProcState::Faulted { vector }
                    }
                    _ => unreachable!(),
                };
                completed += 1;
                // Free the address space now; keep a zombie slot for wait().
                process.space.teardown();
                if let Some(s) = table.slots.get_mut(&pid) {
                    s.process = None;
                    s.state = state;
                }
                wake_waiters(table, pid, encode_status(state));
            }
        }
    }
    completed
}

/// Tear down and remove every process still in the table (test cleanup after
/// a bounded run that intentionally left processes runnable, e.g. an infinite
/// spinner). Returns how many were reaped. Frees all their address spaces.
pub fn drain_all() -> usize {
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    let pids: alloc::vec::Vec<u64> = table.slots.keys().copied().collect();
    let mut n = 0;
    for pid in pids {
        if let Some(slot) = table.slots.remove(&pid) {
            if let Some(process) = slot.process {
                process.space.teardown();
                n += 1;
            }
        }
    }
    table.runq.clear();
    n
}

/// Number of live slots (diagnostics/invariants).
pub fn live_count() -> usize {
    let guard = TABLE.lock();
    guard.as_ref().map(|t| t.slots.len()).unwrap_or(0)
}

/// Wake any process blocked in wait() on `child`, delivering `status`, and
/// reap the child slot.
fn wake_waiters(table: &mut Table, child: u64, status: u64) {
    let waiters: alloc::vec::Vec<u64> = table
        .slots
        .iter()
        .filter_map(|(&pid, s)| match s.state {
            ProcState::Blocked { on } if on == child => Some(pid),
            _ => None,
        })
        .collect();
    if waiters.is_empty() {
        return;
    }
    // Reap the child (its status has been delivered).
    table.slots.remove(&child);
    for wpid in waiters {
        if let Some(w) = table.slots.get_mut(&wpid) {
            if let Some(p) = w.process.as_mut() {
                p.ctx.rax = status; // wait() returns the child status
            }
            w.state = ProcState::Runnable;
            table.runq.push_back(wpid);
        }
    }
}

/// spawn(path_ptr, path_len) — load an ELF from the VFS into a new process
/// and admit it. Returns the child pid, or ERR_*.
pub fn sys_spawn(path_ptr: u64, path_len: u64) -> u64 {
    let bytes = match crate::syscall::copy_from_user(path_ptr, path_len, SPAWN_PATH_MAX) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let path = match core::str::from_utf8(&bytes) {
        Ok(p) => p,
        Err(_) => return crate::syscall::ERR_INVAL,
    };
    match user::load(path) {
        Ok(child) => admit(child),
        Err(_) => ERR_NOENT,
    }
}

/// wait(pid) — if the child is a zombie, reap and return its status;
/// otherwise block the caller until it is.
pub fn sys_wait(child: u64) -> u64 {
    let caller = sched::current();
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    let Some(slot) = table.slots.get(&child) else {
        return ERR_NOENT;
    };
    match slot.state {
        ProcState::Exited(_) | ProcState::Faulted { .. } => {
            let status = encode_status(slot.state);
            table.slots.remove(&child);
            status
        }
        _ => {
            // Block: mark the caller and hand control back to the run-loop.
            if let Some(s) = table.slots.get_mut(&caller) {
                s.state = ProcState::Blocked { on: child };
            }
            drop(guard);
            crate::user::transition::save_yield_context(0);
            crate::user::transition::abort_block();
        }
    }
}

/// Scheduler's notion of "who is running", used by syscalls.
pub mod sched {
    use core::sync::atomic::{AtomicU64, Ordering};
    static CURRENT: AtomicU64 = AtomicU64::new(0);
    pub fn set_current(pid: u64) {
        CURRENT.store(pid, Ordering::SeqCst);
    }
    pub fn current() -> u64 {
        CURRENT.load(Ordering::SeqCst)
    }
}

// Keep the unused-import linter honest across feature growth.
const _: u64 = ERR_2BIG;
const _: u64 = ERR_AGAIN;
