//! Kernel tasks and cooperative round-robin scheduler (B100).
//!
//! V0.1 scope (plan §10.7): task IDs and states, per-task stacks, a real
//! context switch, an idle/boot task, and an explicit yield path. Scheduling
//! is cooperative; the timer interrupt only accounts ticks. Timer-driven
//! preemption is future work and is documented as such.

mod context;
pub mod stack;

use crate::sync::Mutex;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const STACK_SIZE: usize = 32 * 1024;
pub(crate) const MAX_TASKS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Finished,
}

pub struct Task {
    pub id: usize,
    pub name: &'static str,
    pub state: TaskState,
    /// Saved stack pointer while not running.
    rsp: u64,
    /// Entry function, taken exactly once by the trampoline.
    entry: Option<fn()>,
    /// Owned stack storage for heap-allocated stacks; spawned tasks use a
    /// guarded slot in the task-stack window instead (`stack.rs`, V0.10), so
    /// only the boot task's `None` remains. Boxed tasks keep `rsp` slot
    /// addresses stable across Vec growth.
    _stack: Option<Vec<u8>>,
}

struct Scheduler {
    // The boxing is deliberate, not redundant: yield_now/exit_current write
    // the saved RSP through a raw pointer into the Task after releasing the
    // scheduler lock, so Task addresses must survive Vec reallocation.
    #[allow(clippy::vec_box)]
    tasks: Vec<Box<Task>>,
    ready: VecDeque<usize>,
    current: usize,
}

static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);
static SPAWNED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskError {
    NotInitialized,
    TooManyTasks,
    /// The task's guarded stack could not be mapped (V0.10).
    NoStack,
}

/// Initialize the scheduler; the calling (boot) context becomes task 0.
pub fn init() {
    let boot_task = Box::new(Task {
        id: 0,
        name: "kmain",
        state: TaskState::Running,
        rsp: 0,
        entry: None,
        _stack: None,
    });
    let mut tasks = Vec::with_capacity(MAX_TASKS);
    tasks.push(boot_task);
    *SCHEDULER.lock() = Some(Scheduler {
        tasks,
        ready: VecDeque::new(),
        current: 0,
    });
    SPAWNED.store(1, Ordering::SeqCst);
}

/// Spawn a kernel task. It runs when the current task yields.
pub fn spawn(name: &'static str, entry: fn()) -> Result<usize, TaskError> {
    let mut guard = SCHEDULER.lock();
    let sched = guard.as_mut().ok_or(TaskError::NotInitialized)?;
    if sched.tasks.len() >= MAX_TASKS {
        return Err(TaskError::TooManyTasks);
    }

    let id = sched.tasks.len();
    // A guarded stack (V0.10): an overflow faults on the unmapped page below
    // it instead of silently overwriting the heap.
    let stack_top = stack::map(id).map_err(|_| TaskError::NoStack)? & !0xF;

    // Seed the stack so the first context switch "returns" into the
    // trampoline: [6 zeroed callee-saved regs][trampoline address].
    let trampoline: extern "C" fn() -> ! = context::task_trampoline;
    let rsp = unsafe {
        let mut sp = stack_top as *mut u64;
        sp = sp.sub(1);
        sp.write(trampoline as usize as u64);
        for _ in 0..6 {
            sp = sp.sub(1);
            sp.write(0);
        }
        sp as u64
    };

    sched.tasks.push(Box::new(Task {
        id,
        name,
        state: TaskState::Ready,
        rsp,
        entry: Some(entry),
        _stack: None,
    }));
    sched.ready.push_back(id);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    Ok(id)
}

/// Cooperatively yield to the next ready task (no-op when none).
pub fn yield_now() {
    let (old_slot, new_rsp);
    {
        let mut guard = SCHEDULER.lock();
        let Some(sched) = guard.as_mut() else { return };
        let Some(next) = sched.ready.pop_front() else {
            return;
        };
        let cur = sched.current;
        sched.tasks[cur].state = TaskState::Ready;
        sched.ready.push_back(cur);
        sched.tasks[next].state = TaskState::Running;
        sched.current = next;
        old_slot = &mut sched.tasks[cur].rsp as *mut u64;
        new_rsp = sched.tasks[next].rsp;
        // Guard dropped before switching; Boxed tasks keep old_slot stable.
    }
    // SAFETY: old_slot points into a boxed Task that outlives the switch;
    // new_rsp was seeded by spawn() or saved by a previous switch.
    unsafe { context::switch_context(old_slot, new_rsp) }
}

/// Called by the trampoline: fetch the entry function of the current task.
pub(crate) fn take_current_entry() -> Option<fn()> {
    let mut guard = SCHEDULER.lock();
    let sched = guard.as_mut()?;
    let cur = sched.current;
    sched.tasks[cur].entry.take()
}

/// Terminate the current task and switch away forever.
pub(crate) fn exit_current() -> ! {
    loop {
        let mut switched = false;
        {
            let mut guard = SCHEDULER.lock();
            if let Some(sched) = guard.as_mut() {
                let cur = sched.current;
                sched.tasks[cur].state = TaskState::Finished;
                if let Some(next) = sched.ready.pop_front() {
                    sched.tasks[next].state = TaskState::Running;
                    sched.current = next;
                    let old_slot = &mut sched.tasks[cur].rsp as *mut u64;
                    let new_rsp = sched.tasks[next].rsp;
                    drop(guard);
                    // SAFETY: as in yield_now; the finished task's rsp slot
                    // is still valid storage, it just never runs again.
                    unsafe { context::switch_context(old_slot, new_rsp) }
                    // Unreachable in practice: nothing re-queues a Finished
                    // task. Loop defensively regardless.
                    switched = true;
                }
            }
        }
        if !switched {
            // No ready task to switch to — should not happen because task 0
            // never exits; halt rather than corrupt state.
            x86_64::instructions::hlt();
        }
    }
}

/// Snapshot of task diagnostics for the shell.
pub fn task_list() -> Vec<(usize, &'static str, TaskState)> {
    let guard = SCHEDULER.lock();
    match guard.as_ref() {
        Some(sched) => sched
            .tasks
            .iter()
            .map(|t| (t.id, t.name, t.state))
            .collect(),
        None => Vec::new(),
    }
}

/// True when every spawned task except the boot task has finished.
pub fn all_spawned_finished() -> bool {
    let guard = SCHEDULER.lock();
    match guard.as_ref() {
        Some(sched) => sched
            .tasks
            .iter()
            .skip(1)
            .all(|t| t.state == TaskState::Finished),
        None => true,
    }
}
