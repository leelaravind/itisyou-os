# ADR-0006: Concurrent processes — resumable contexts + cooperative run-loop

**Status:** Accepted · 2026-09-02 (V0.3)

## Context

V0.2's transition ran a process to completion (exit/fault only). Multiple
Ring 3 processes need to interleave, which requires *resuming* a process
where it left off.

## Decision

- **Unified entry/resume via `sysretq`**: one code path (`enter_user_raw`)
  serves both the first entry and every resume. It loads RIP/RFLAGS/RSP and
  callee-saved registers from a `#[repr(C)]` [`UserContext`], zeroing scratch
  registers so no kernel data reaches Ring 3.
- **Resumable context**: only callee-saved GPRs + RIP/RSP/RFLAGS are tracked
  — userspace leaves solely via the `syscall` instruction (a function call,
  caller-saved regs already dead) or a fault (process terminates). The
  syscall entry stub snapshots this context before the dispatcher runs.
- **Outcomes**: `yield` → `Yielded` (re-queued immediately); `wait` on a live
  child → `Blocked` (re-queued only when woken, delivering the child status
  in rax); `exit`/fault → terminal. All long-jump back to the run-loop via a
  saved setjmp-style abort context.
- **Process table + run-loop** (`proc.rs`): the run-loop owns every
  `Process`; it takes one out of the table for a quantum (so `spawn`/`wait`
  can lock the table re-entrantly), runs it, and updates state. Round-robin
  over the run queue. Zombies hold status for `wait`; teardown frees the
  address space at exit.
- **Scheduling is cooperative**: the PIT still ticks at CPL=3 but does not
  preempt user code. Preemptive user scheduling needs full trap-frame
  save/restore in the timer ISR and is deferred (KNOWN_LIMITATIONS).

## Alternatives

- **Kernel-thread-per-process** on the existing V0.1 task switcher: heavier,
  and the resumable-context approach reuses the syscall fast path.
- **Preemptive from day one**: larger, riskier; cooperative first proves the
  model with deterministic, adversarially-testable interleaving.

## Consequences

- Deterministic interleaving is directly observable (both children print
  before either exits) and asserted by the harness.
- A cooperative process that never yields would monopolize the CPU — a known
  limitation until preemption lands.
