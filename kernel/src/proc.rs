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
                // The process's last partial line comes before the kernel's
                // exit report.
                crate::console_out::flush_owner(pid);
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
                // Release everything it owned, then free its address space;
                // keep a zombie slot for wait().
                release_owned(pid);
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

/// Release everything a process owned, on EVERY exit path (V0.10): the
/// scheduler's terminal path, `reap`, `drain_all` and the console's
/// foreground `user::run`. Before V0.10 each path had its own list, and two
/// of them forgot the process's windows (a leak: pids are never reused, so a
/// leaked window could never be removed). Output is flushed first so a
/// process's last partial line appears before anything the kernel says
/// about the exit.
pub fn release_owned(pid: u64) {
    crate::console_out::flush_owner(pid);
    crate::gfx::compositor::remove_owned(pid);
    crate::capability::revoke_owner(pid);
    // A dead program must not leave a UDP port bound or a TCP connection
    // owned: the port would stay unusable for the rest of the boot.
    crate::net::socket::close_owner(pid);
    crate::net::tcp::close_owner(pid);
}

/// Run the scheduler until nothing is runnable OR `max_ticks` elapse —
/// the service supervisor's bounded scheduling slice (V0.7).
pub fn run_until_pid_idle_bounded(max_ticks: u64) {
    let deadline = crate::interrupts::ticks() + max_ticks;
    run_scheduler(|_| crate::interrupts::ticks() >= deadline);
}

/// Remove one process's slot (V0.7 supervisor cleanup): tears down its
/// address space if it never reached a terminal state. No waiters are woken —
/// the caller owns this pid's lifecycle.
pub fn reap(pid: u64) {
    let mut guard = TABLE.lock();
    if let Some(table) = guard.as_mut() {
        if let Some(slot) = table.slots.remove(&pid) {
            if let Some(process) = slot.process {
                release_owned(pid);
                process.space.teardown();
            }
        }
    }
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
                release_owned(pid);
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
/// and admit it. Returns the child pid, or ERR_*. The child INHERITS the
/// parent's capabilities and FS sandbox exactly (V0.7) — spawning can never
/// amplify authority.
pub fn sys_spawn(path_ptr: u64, path_len: u64) -> u64 {
    spawn_common(path_ptr, path_len, u64::MAX, user::Args::empty(), "spawn")
}

/// spawn_caps(path_ptr, path_len, requested) — like spawn, but the child
/// receives `parent caps ∩ requested` (controlled delegation; requesting more
/// than the parent holds silently yields only the intersection — a child can
/// NEVER hold what its parent lacked).
pub fn sys_spawn_caps(path_ptr: u64, path_len: u64, requested: u64) -> u64 {
    spawn_common(path_ptr, path_len, requested, user::Args::empty(), "spawn")
}

/// Size of the `spawn_args` request: `[requested:8, args_ptr:8, args_len:8]`.
const SPAWN_ARGS_REQ: u64 = 24;

/// spawn_args(path_ptr, path_len, req_ptr) — `spawn_caps` plus an argument
/// block for the child (V0.10). `req` is `[requested:8, args_ptr:8,
/// args_len:8]`, little-endian; the syscall ABI has three argument registers,
/// so the extra operands travel in memory like `fs_read`'s request.
///
/// Delegation is exactly `spawn_caps`'s (the Process capability was checked
/// by the dispatcher). The block is refused BEFORE anything is loaded, with
/// the same validator the console uses: more than 16 arguments or more than
/// 512 bytes is `ERR_2BIG` (POSIX's "argument list too long"); an empty
/// argument, a space or non-printable byte, or a missing terminator is
/// `ERR_INVAL`. A block longer than 512 bytes is refused without being copied
/// at all.
pub fn sys_spawn_args(path_ptr: u64, path_len: u64, req_ptr: u64) -> u64 {
    use kernel_core::progargs::{ArgError, MAX_BLOCK};
    let req = match crate::syscall::copy_from_user(req_ptr, SPAWN_ARGS_REQ, SPAWN_ARGS_REQ) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let word = |i: usize| u64::from_le_bytes(req[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
    let (requested, args_ptr, args_len) = (word(0), word(1), word(2));
    let block = match crate::syscall::copy_from_user(args_ptr, args_len, MAX_BLOCK as u64) {
        Ok(b) => b,
        Err(e) => {
            if e == ERR_2BIG {
                crate::serial_println!(
                    "[ITISYOU:INFO] spawn_args refused reason={} len={args_len}",
                    ArgError::TooLong.name()
                );
            }
            return e;
        }
    };
    let args = match user::Args::new(&block) {
        Ok(a) => a,
        Err(err) => {
            crate::serial_println!(
                "[ITISYOU:INFO] spawn_args refused reason={} len={args_len}",
                err.name()
            );
            return match err {
                ArgError::TooMany | ArgError::TooLong => ERR_2BIG,
                ArgError::Empty | ArgError::BadByte | ArgError::Unterminated => {
                    crate::syscall::ERR_INVAL
                }
            };
        }
    };
    spawn_common(path_ptr, path_len, requested, args, "spawn_args")
}

fn spawn_common(
    path_ptr: u64,
    path_len: u64,
    requested: u64,
    args: user::Args,
    action: &'static str,
) -> u64 {
    let bytes = match crate::syscall::copy_from_user(path_ptr, path_len, SPAWN_PATH_MAX) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let path = match core::str::from_utf8(&bytes) {
        Ok(p) => p,
        Err(_) => return crate::syscall::ERR_INVAL,
    };
    let parent_caps = crate::syscall::CURRENT_CAPS.load(core::sync::atomic::Ordering::SeqCst);
    let child_caps = kernel_core::caps::delegate(parent_caps, requested);
    // The child inherits the parent's FS sandbox (it can only stay as tight).
    let sandbox = crate::syscall::current_sandbox_for_child();
    match user::load_with(path, child_caps, sandbox) {
        Ok(mut child) => {
            // V0.8: the child's handles are DELEGATED from the parent's live
            // handles rather than minted fresh from the intersected bitmask.
            // The bit intersection above decides what is asked for; the table
            // decides what can actually be handed over, so a parent whose own
            // authority was revoked or has expired cannot pass it on, and
            // amplification is refused by the same code that enforces every
            // other check.
            let parent_pid = crate::syscall::CURRENT_PID.load(core::sync::atomic::Ordering::SeqCst);
            // `load_with` minted a set straight from the bitmask; drop it
            // before installing the delegated one, so the child never holds
            // two sets (and the table never leaks the discarded slots).
            crate::capability::revoke_owner(child.pid);
            child.handles = crate::capability::delegate_to_child(
                parent_pid,
                child.pid,
                child_caps,
                crate::interrupts::ticks(),
            );
            let argc = args.count();
            child.args = args;
            let pid = admit(child);
            let detail = if action == "spawn" {
                alloc::string::String::from(path)
            } else {
                alloc::format!("{path} argc={argc}")
            };
            crate::audit::allowed(action, child_caps, Some(detail));
            pid
        }
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
