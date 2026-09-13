//! Process table and concurrent-process scheduler (V0.3, ADR-0006; process
//! tree V0.10, PROC10-002).
//!
//! Multiple Ring 3 processes run cooperatively on the resumable V0.2/V0.3
//! transition. The run-loop owns every [`Process`]; it takes one out of the
//! table for a quantum (so `spawn`/`wait` can lock the table re-entrantly),
//! runs it until it yields / blocks / exits / faults, then updates state.
//! `yield` round-robins; `wait` blocks the caller until the child is a
//! zombie, delivering the child's status as the syscall return value.
//!
//! V0.10: every process has a parent (0 = the console), and only the parent
//! may collect a child (`wait`, `wait_nohang`). When a process dies its
//! children go to the registered adopter (`/sbin/init`, once it runs) or,
//! with none, a live child is reaped automatically when it ends and a dead
//! one is removed at once. `sleep` parks a process until a tick; the
//! run-loop wakes it. The console can `kill` a process and list them (`ps`).

use crate::sync::Mutex;
use crate::syscall::{ERR_2BIG, ERR_AGAIN, ERR_INVAL, ERR_NOENT};
use crate::user::{self, Process, UserExit};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::procstatus::{self, Status};
use kernel_core::proctree::{self, Adopt, ORPHAN_PARENT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcState {
    Runnable,
    /// Blocked in wait() on the given child pid.
    Blocked {
        on: u64,
    },
    /// Parked by `sleep` until this tick (V0.10).
    Sleeping {
        until: u64,
    },
    Exited(u64),
    Faulted {
        vector: u8,
    },
    /// Terminated by the console's `kill` (V0.10).
    Killed,
}

impl ProcState {
    /// Has the process ended (a zombie until its parent collects it)?
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            ProcState::Exited(_) | ProcState::Faulted { .. } | ProcState::Killed
        )
    }
}

struct Slot {
    /// Present except while this process is out running its quantum.
    process: Option<Process>,
    state: ProcState,
    started: bool,
    /// Who may collect this process: its parent's pid, 0 for the console, or
    /// [`ORPHAN_PARENT`] when the parent died with no adopter (V0.10).
    parent: u64,
    /// What it runs, for `ps`.
    path: String,
    /// Where it faulted, if it did (the console's `run` reports it, V0.10).
    fault_addr: Option<u64>,
    /// The capability bits it was launched with (what `svc_report` prints,
    /// V0.10 — the kernel's record, not a supervisor's claim).
    caps: u64,
}

struct Table {
    slots: BTreeMap<u64, Slot>,
    runq: VecDeque<u64>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

/// The adopter of orphans: `/sbin/init` registers itself here once it runs
/// (V0.10, S11). 0 = none, so orphans are reaped automatically.
pub static ADOPT_PID: AtomicU64 = AtomicU64::new(0);

/// The longest `sleep`, in ticks (60 s at 100 Hz).
pub const SLEEP_MAX_TICKS: u64 = 6000;

const SPAWN_PATH_MAX: u64 = 128;

pub fn init() {
    *TABLE.lock() = Some(Table {
        slots: BTreeMap::new(),
        runq: VecDeque::new(),
    });
}

/// Admit an already-loaded process as RUNNABLE, a child of the console.
/// Returns its pid.
pub fn admit(process: Process) -> u64 {
    admit_child(process, 0)
}

/// Admit a process as RUNNABLE, a child of `parent` (V0.10).
pub fn admit_child(process: Process, parent: u64) -> u64 {
    let pid = process.pid;
    let path = process.path.clone();
    let caps = process.caps;
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    table.slots.insert(
        pid,
        Slot {
            process: Some(process),
            state: ProcState::Runnable,
            started: false,
            parent,
            path,
            fault_addr: None,
            caps,
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

/// The launch capabilities of `pid` if it is an uncollected child of
/// `parent` (running or ended), for `svc_report` (V0.10).
pub fn child_caps(parent: u64, pid: u64) -> Option<u64> {
    let guard = TABLE.lock();
    let slot = guard.as_ref()?.slots.get(&pid)?;
    proctree::may_wait(slot.parent, parent).then_some(slot.caps)
}

/// Is `pid` gone or ended (a supervisor that can no longer report)?
pub fn is_dead(pid: u64) -> bool {
    !matches!(state_of(pid), Some(s) if !s.is_terminal())
}

/// Where a faulted process faulted, if the CPU reported an address.
pub fn fault_addr_of(pid: u64) -> Option<u64> {
    let guard = TABLE.lock();
    guard.as_ref()?.slots.get(&pid).and_then(|s| s.fault_addr)
}

/// What owns a resource, as the `tcp` listing reports it (V0.10): the
/// console, an orphan (its process died and the connection is closing on
/// its own), a live process, or — which must never happen, because every
/// exit path releases what the process owned — a dead one.
pub fn owner_state(owner: u64) -> &'static str {
    if owner == 0 {
        return "console";
    }
    if owner == crate::net::tcp::ORPHAN {
        return "orphan";
    }
    match state_of(owner) {
        Some(s) if !s.is_terminal() => "live",
        _ => "dead",
    }
}

/// The status word `wait` delivers (`kernel_core::procstatus`).
fn encode_status(state: ProcState) -> u64 {
    match state {
        ProcState::Exited(code) => procstatus::encode(Status::Exited(code as u32)),
        ProcState::Faulted { vector } => procstatus::encode(Status::Faulted(vector)),
        ProcState::Killed => procstatus::KILLED,
        _ => 0,
    }
}

/// Run the scheduler until no process is runnable (sleepers are waited
/// for). Returns how many reached a terminal state. A leftover Blocked
/// process with no runnable peer is a deadlock and is reported (should not
/// happen in the tested programs).
pub fn run_until_idle() -> usize {
    run_scheduler(|_| false, true)
}

/// Run the scheduler preemptively until `target` reaches a terminal state or
/// `max_ticks` timer ticks elapse. Returns `target`'s state (if any) — used
/// by the non-monopolization proof, where a co-scheduled infinite spinner
/// keeps the run queue non-empty forever. `run_until_idle` cannot be used
/// there because it only stops when nothing is runnable.
pub fn run_until_pid_exits(target: u64, max_ticks: u64) -> Option<ProcState> {
    let deadline = crate::interrupts::ticks() + max_ticks;
    run_scheduler(
        |_| {
            let done = !matches!(state_of(target), Some(s) if !s.is_terminal());
            done || crate::interrupts::ticks() >= deadline
        },
        true,
    );
    state_of(target)
}

/// Set while a run-loop is running processes. The Ring 3 transition state is
/// single-slot, so a second run-loop (a safe point reached from inside a
/// slice, say) would corrupt it: it is refused outright (V0.10, R3).
static RUNLOOP_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn runloop_active() -> bool {
    RUNLOOP_ACTIVE.load(Ordering::SeqCst)
}

/// Clears [`RUNLOOP_ACTIVE`] however the run-loop returns.
struct RunLoopGuard;

impl Drop for RunLoopGuard {
    fn drop(&mut self) {
        RUNLOOP_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// One bounded background slice (V0.10): run processes until `max_quanta`
/// quanta, `max_ticks` ticks or `max_ms` of wall time (TSC — it keeps
/// counting while interrupts are masked), whichever comes first. The bounds
/// are checked between quanta, so a slice overruns by at most one quantum.
/// A slice never waits for a sleeper: it returns when nothing is runnable.
/// Returns the quanta it ran.
pub fn run_slice(max_ticks: u64, max_quanta: u32, max_ms: u64) -> u32 {
    let start_q = crate::sched::quanta_total();
    let tick_deadline = crate::interrupts::ticks() + max_ticks;
    let tsc_deadline = crate::interrupts::tsc() + crate::interrupts::cycles_for_ms(max_ms);
    run_scheduler(
        |_| {
            crate::sched::quanta_total() - start_q >= max_quanta as u64
                || crate::interrupts::ticks() >= tick_deadline
                || crate::interrupts::tsc() >= tsc_deadline
        },
        false,
    );
    (crate::sched::quanta_total() - start_q) as u32
}

/// Core scheduler loop. Runs each runnable process for one quantum (until it
/// yields, is preempted, blocks, exits, or faults), updating state. Stops
/// when `stop(completed)` returns true, or when nothing is runnable — unless
/// `wait_for_sleepers` and a process is sleeping, in which case it halts
/// until the next interrupt and looks again (V0.10).
fn run_scheduler(stop: impl Fn(usize) -> bool, wait_for_sleepers: bool) -> usize {
    assert!(
        !RUNLOOP_ACTIVE.swap(true, Ordering::SeqCst),
        "run-loop re-entered"
    );
    let _active = RunLoopGuard;
    let mut completed = 0;
    loop {
        if stop(completed) {
            break;
        }
        let (taken, sleepers) = {
            let mut guard = TABLE.lock();
            let table = guard.as_mut().expect("proc table init");
            let sleepers = wake_due_sleepers(table, crate::interrupts::ticks());
            let taken = loop {
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
            };
            (taken, sleepers)
        };
        let Some((pid, mut process, first)) = taken else {
            if wait_for_sleepers && sleepers && x86_64::instructions::interrupts::are_enabled() {
                // Nothing runnable, but a sleeper will be: wait for the next
                // tick (the timer interrupt ends the halt).
                x86_64::instructions::hlt();
                continue;
            }
            crate::sched::note_runq_empty();
            break;
        };

        crate::sched::note_quantum(pid);
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
                // wait() or sleep() already set this slot's state.
                if let Some(s) = table.slots.get_mut(&pid) {
                    s.process = Some(process);
                }
            }
            UserExit::Exit(code) => {
                completed += 1;
                finish(table, pid, Some(process), ProcState::Exited(code), None);
            }
            UserExit::Fault { vector, addr } => {
                completed += 1;
                finish(
                    table,
                    pid,
                    Some(process),
                    ProcState::Faulted { vector },
                    addr,
                );
            }
        }
    }
    completed
}

/// A process has ended (exit, fault, or `kill`): the one path every
/// termination takes (V0.10). Its last partial line, then the kernel's
/// report; everything it owned released and its address space freed; its
/// children handed on; its parent (if waiting) woken with the status; and
/// the slot removed outright if nobody will ever collect it.
fn finish(
    table: &mut Table,
    pid: u64,
    process: Option<Process>,
    state: ProcState,
    fault_addr: Option<u64>,
) {
    crate::console_out::flush_owner(pid);
    match state {
        ProcState::Exited(code) => {
            crate::serial_println!("[ITISYOU:INFO] user_exit pid={pid} code={code}");
        }
        ProcState::Faulted { vector } => crate::serial_println!(
            "[ITISYOU:INFO] user_fault pid={pid} vector={vector} addr={:#x} contained=true",
            fault_addr.unwrap_or(0)
        ),
        ProcState::Killed => crate::serial_println!("[ITISYOU:PROC] killed pid={pid}"),
        _ => {}
    }
    release_owned(pid);
    if let Some(p) = process {
        p.space.teardown();
    }
    let parent = match table.slots.get_mut(&pid) {
        Some(s) => {
            s.process = None;
            s.state = state;
            s.fault_addr = fault_addr;
            s.parent
        }
        None => ORPHAN_PARENT,
    };
    adopt_orphans(table, pid);
    wake_waiters(table, pid, encode_status(state));
    if parent == ORPHAN_PARENT {
        table.slots.remove(&pid);
    }
}

/// Release everything a process owned, on EVERY exit path (V0.10): the
/// scheduler's terminal path, `kill`, `reap`, `drain_all` and the console's
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
    run_scheduler(|_| crate::interrupts::ticks() >= deadline, true);
}

/// Remove one process's slot (V0.7 supervisor cleanup): tears down its
/// address space if it never reached a terminal state. No waiters are woken —
/// the caller owns this pid's lifecycle. Its children are handed on exactly
/// as if it had died (V0.10), so none is left with a parent that no longer
/// exists.
pub fn reap(pid: u64) {
    let mut guard = TABLE.lock();
    if let Some(table) = guard.as_mut() {
        if let Some(slot) = table.slots.remove(&pid) {
            if let Some(process) = slot.process {
                release_owned(pid);
                process.space.teardown();
            }
            adopt_orphans(table, pid);
        }
    }
}

/// The console's foreground `run` ended its program, which is not in the
/// table: hand on any children it spawned (V0.10).
pub fn orphan_children_of(pid: u64) {
    let mut guard = TABLE.lock();
    if let Some(table) = guard.as_mut() {
        adopt_orphans(table, pid);
    }
}

/// Tear down and remove every process still in the table (test cleanup after
/// a bounded run that intentionally left processes runnable, e.g. an infinite
/// spinner). Returns how many were reaped. Frees all their address spaces.
pub fn drain_all() -> usize {
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    let pids: Vec<u64> = table.slots.keys().copied().collect();
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
    let waiters: Vec<u64> = table
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

/// Hand the children of `dying` on (V0.10): to the adopter if one is
/// registered and alive; otherwise a child that already ended is removed now
/// and a live one is marked to be removed when it ends.
fn adopt_orphans(table: &mut Table, dying: u64) {
    let target = match proctree::adopt_target(ADOPT_PID.load(Ordering::SeqCst), dying) {
        Adopt::Init(init)
            if table
                .slots
                .get(&init)
                .is_some_and(|s| !s.state.is_terminal()) =>
        {
            Adopt::Init(init)
        }
        _ => Adopt::AutoReap,
    };
    let children: Vec<u64> = table
        .slots
        .iter()
        .filter(|(_, s)| s.parent == dying)
        .map(|(&pid, _)| pid)
        .collect();
    for child in children {
        match target {
            Adopt::Init(init) => {
                if let Some(s) = table.slots.get_mut(&child) {
                    s.parent = init;
                }
                crate::serial_println!(
                    "[ITISYOU:PROC] reparent child={child} from={dying} to={init}"
                );
            }
            Adopt::AutoReap => {
                let ended = table
                    .slots
                    .get(&child)
                    .is_some_and(|s| s.state.is_terminal());
                if ended {
                    table.slots.remove(&child);
                    crate::serial_println!(
                        "[ITISYOU:PROC] reaped child={child} from={dying} reason=orphaned_zombie"
                    );
                } else {
                    if let Some(s) = table.slots.get_mut(&child) {
                        s.parent = ORPHAN_PARENT;
                    }
                    crate::serial_println!(
                        "[ITISYOU:PROC] reparent child={child} from={dying} to=orphan"
                    );
                }
            }
        }
    }
}

/// Make every sleeper whose tick has come runnable again. Returns whether any
/// process is still sleeping.
fn wake_due_sleepers(table: &mut Table, now: u64) -> bool {
    let Table { slots, runq } = table;
    let mut still = false;
    for (&pid, s) in slots.iter_mut() {
        if let ProcState::Sleeping { until } = s.state {
            if proctree::due(until, now) {
                s.state = ProcState::Runnable;
                runq.push_back(pid);
            } else {
                still = true;
            }
        }
    }
    still
}

/// What the console's `kill` did.
pub enum KillResult {
    NoSuch,
    AlreadyTerminated,
    Killed { path: String },
}

/// Terminate a process from the console (V0.10). Only between run-loops:
/// then every process's state is in its slot, none is mid-quantum.
pub fn kill(pid: u64) -> KillResult {
    assert!(!runloop_active(), "kill from inside a run-loop");
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    let Some(slot) = table.slots.get_mut(&pid) else {
        return KillResult::NoSuch;
    };
    if slot.state.is_terminal() {
        return KillResult::AlreadyTerminated;
    }
    let process = slot.process.take();
    let path = slot.path.clone();
    finish(table, pid, process, ProcState::Killed, None);
    KillResult::Killed { path }
}

/// Every process in the table, lowest pid first: pid, parent, path, state
/// (for `ps`).
pub fn for_each(mut f: impl FnMut(u64, u64, &str, ProcState)) {
    let guard = TABLE.lock();
    if let Some(table) = guard.as_ref() {
        for (&pid, s) in &table.slots {
            f(pid, s.parent, &s.path, s.state);
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
                ArgError::Empty | ArgError::BadByte | ArgError::Unterminated => ERR_INVAL,
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
        Err(_) => return ERR_INVAL,
    };
    let parent_caps = crate::syscall::CURRENT_CAPS.load(Ordering::SeqCst);
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
            let parent_pid = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
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
            // V0.10: the spawner is the parent — the only process that may
            // collect this child.
            let pid = admit_child(child, parent_pid);
            let detail = if action == "spawn" {
                String::from(path)
            } else {
                alloc::format!("{path} argc={argc}")
            };
            crate::audit::allowed(action, child_caps, Some(detail));
            pid
        }
        Err(_) => ERR_NOENT,
    }
}

/// What `wait`/`wait_nohang` found for the caller.
enum Found {
    /// This child has ended: collect it.
    Reap(u64),
    /// Matching children exist but none has ended.
    Alive,
    /// No such process, or no children at all.
    Missing,
    /// The process exists but is not the caller's child.
    Foreign,
}

/// Look for `pid` (0 = any child of the caller, lowest pid first).
fn find_child(table: &Table, caller: u64, pid: u64) -> Found {
    if pid == 0 {
        let children: Vec<(u64, bool)> = table
            .slots
            .iter()
            .filter(|(_, s)| proctree::may_wait(s.parent, caller))
            .map(|(&p, s)| (p, s.state.is_terminal()))
            .collect();
        if children.is_empty() {
            return Found::Missing;
        }
        return match proctree::pick_reapable(children) {
            Some(p) => Found::Reap(p),
            None => Found::Alive,
        };
    }
    match table.slots.get(&pid) {
        None => Found::Missing,
        Some(s) if !proctree::may_wait(s.parent, caller) => Found::Foreign,
        Some(s) if s.state.is_terminal() => Found::Reap(pid),
        Some(_) => Found::Alive,
    }
}

/// A process tried to collect a process that is not its child (V0.10).
fn refuse_foreign() -> u64 {
    crate::audit::denied("wait_foreign", kernel_core::caps::CAP_SPAWN);
    ERR_NOENT
}

/// wait(pid) — if the child is a zombie, reap and return its status;
/// otherwise block the caller until it is. Only the parent may wait
/// (V0.10): for any other process — even one that exists — the answer is
/// `ERR_NOENT`, and the attempt is audited.
pub fn sys_wait(child: u64) -> u64 {
    // The syscall layer's pid: under the console's foreground `run` the
    // run-loop's notion of "current" is the console, not the caller.
    let caller = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    match find_child(table, caller, child) {
        Found::Missing => ERR_NOENT,
        Found::Foreign => {
            drop(guard);
            refuse_foreign()
        }
        Found::Reap(pid) => table
            .slots
            .remove(&pid)
            .map_or(0, |s| encode_status(s.state)),
        Found::Alive => {
            // Block: mark the caller and hand control back to the run-loop,
            // which delivers the status when the child ends. A foreground
            // program (no slot, no run-loop) is simply resumed and gets 0.
            if let Some(s) = table.slots.get_mut(&caller) {
                s.state = ProcState::Blocked { on: child };
            }
            drop(guard);
            crate::user::transition::save_yield_context(0);
            crate::user::transition::abort_block();
        }
    }
}

/// wait_nohang(pid, status_out) — collect an ended child WITHOUT blocking
/// (V0.10, syscall 37). `pid = 0` means any child of the caller, lowest pid
/// first. Returns the collected child's pid and writes its 8-byte status to
/// `status_out`; `ERR_AGAIN` if the matching children are all still running;
/// `ERR_NOENT` if there is no matching child, or the process is not the
/// caller's child (audited). `status_out` is checked first, so a status that
/// could not be delivered never costs the caller its child.
pub fn sys_wait_nohang(pid: u64, status_out: u64) -> u64 {
    if let Err(e) = crate::syscall::validate_user_write(status_out, 8) {
        return e;
    }
    let caller = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    let mut guard = TABLE.lock();
    let table = guard.as_mut().expect("proc table init");
    let target = match find_child(table, caller, pid) {
        Found::Reap(p) => p,
        Found::Alive => return ERR_AGAIN,
        Found::Missing => return ERR_NOENT,
        Found::Foreign => {
            drop(guard);
            return refuse_foreign();
        }
    };
    let status = table
        .slots
        .remove(&target)
        .map_or(0, |s| encode_status(s.state));
    drop(guard);
    match crate::syscall::copy_to_user(status_out, &status.to_le_bytes()) {
        Ok(_) => target,
        Err(e) => e,
    }
}

/// sleep(ticks) — park the caller for `ticks` timer ticks (V0.10, syscall
/// 38; no capability: a process can only ever delay itself). At most
/// [`SLEEP_MAX_TICKS`], else `ERR_INVAL`; 0 behaves like `yield`. Nothing
/// blocks inside the syscall (R4): the caller is marked and the run-loop
/// wakes it when the tick comes. A foreground program (no slot) is resumed
/// at once — the console's `run` has no run-loop to wake it.
pub fn sys_sleep(ticks: u64) -> u64 {
    if ticks > SLEEP_MAX_TICKS {
        return ERR_INVAL;
    }
    crate::user::transition::save_yield_context(0);
    if ticks == 0 {
        crate::user::transition::abort_yield();
    }
    let caller = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    let until = proctree::deadline(crate::interrupts::ticks(), ticks);
    if let Some(s) = TABLE.lock().as_mut().and_then(|t| t.slots.get_mut(&caller)) {
        s.state = ProcState::Sleeping { until };
    }
    crate::user::transition::abort_block();
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
