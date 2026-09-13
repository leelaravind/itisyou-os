# ADR-0022: Always-on co-scheduling at audited safe points

## Status

Accepted for V0.10 (SCHED10-001). Implemented in two steps: the core built,
measured and disabled (S4), then enabled at the safe points (S5).

## Context

Before V0.10, Ring 3 processes ran only when the kernel console drove the
scheduler: at the idle prompt, during `bg`, and inside the supervisor. A
long-running console command — a network wait, the desktop, a foreground
`run` — starved every background process for its whole duration. A client
run in the foreground could never talk to a service, because the service was
not running while the client was. A userspace `init` and a Ring 3 shell need
background processes that keep running whatever the console is doing.

The kernel is single-CPU and not preemptible. The Ring 3 transition state is
single-slot (one saved user context, one syscall snapshot, one current pid,
one quantum counter). Syscalls run with interrupts masked. Storage, the NIC
and most device drivers assume one owner at a time.

Three designs were weighed:

1. **Schedule from the timer interrupt.** Rejected: the timer ISR would have to
   switch away from arbitrary kernel code — inside a syscall, holding a device
   lock, in the middle of the single-slot transition state — which needs a
   preemptible kernel with per-task transition state first.
2. **Run every process as a kernel task.** Rejected for V0.10: it needs
   per-task Ring 3 state, per-task syscall stacks and a lock discipline audit
   of every driver; S2 (counted locks) and S3 (the shared task-stack window)
   are the groundwork for it, not the whole of it.
3. **Bounded slices at audited safe points** (chosen).

## Decision

Background processes get the CPU from **bounded slices**, run only at a few
named **safe points** in kernel code:

- `sched::idle_point()` — the console prompt (no period);
- `sched::console_wait_step(job)` — `bg` job waits (the job only ever runs in
  slices, so this point ignores enable, pause, line position and period);
- `sched::safe_point()` — busy waits: every `net::poll` (ping, resolve, dhcp,
  ipv6, `net poll`, `tcp serve`, audit anchor), the desktop and `usbwait`
  loops, the `busy` diagnostic, and between two quanta of a foreground `run`
  (until S7 moves `run` into the process table).

A slice is `proc::run_slice(1 tick, 32 quanta, 20 ms)`: whichever cap is
reached first ends it (the bounds are checked between quanta, so a slice
overruns by at most one quantum). The policy is the pure, host-tested
`kernel_core::cosched::decide`, checked in a fixed order so a refused slice
always reports its most fundamental reason:

1. interrupts enabled (never from a syscall or an interrupt handler);
2. no run-loop already running processes (`run_scheduler` panics if
   re-entered);
3. no Ring 3 quantum live (`CURRENT_PID == 0`);
4. no counted kernel lock held (`sync::depth() == 0`, S2);
5. no non-schedulable region open (`sched::NoSched`; an open NVMe store holds
   one from `init` to `Drop`);
6. then, for idle and busy points, enabled and not paused;
7. then, for busy points only, the UART at the start of a line and at least
   5 ticks since the last slice.

Rules that bind every change (R1–R14 of the V0.10 plan):

- **R1** Slices only at the named safe points. Adding one needs an audit note
  in the code and in this ADR.
- **R3** One Ring 3 entry at a time: the single-slot state is touched only by
  one run-loop frame or one foreground `run` frame, never both at once; a
  foreground program is out of Ring 3, its state saved, whenever its console
  loop reaches the safe point.
- **R4** Nothing blocks inside a syscall.
- **R5** The process table is never held across a quantum.
- **R6** Lock order: table → console output → serial; table →
  compositor/capability/socket/tcp. The scheduler itself uses atomics only.
- **R7** Storage is single-owner: an open store is a non-schedulable region,
  so every store, package and audit transaction runs without slices, and
  `pkg launch` closes the store before the app runs.
- **R8** `net::poll` is a safe point; no caller may hold a lock across it (a
  held lock makes the point refuse, counted as `lock_skips`).
- **R9** Ring 3 output is line-atomic (OUT10-001) and a busy point refuses
  while the console is mid-line, so background output never splices into a
  kernel line.
- **R10** Explicitly non-schedulable, bounded and documented: every syscall
  (`net_resolve` can take 1.5 s), `irq` (about 200 ms with the timer masked),
  `xhciwait` (up to 5 s inside the controller lock), `beep`, and every
  NVMe-holding command.
- **R11** The selftest image never enables it: its checks count every process
  step.
- **R12** Every exit path releases what the process owned (PROC10-003).

**Measured, not assumed.** `sched` reports slices per kind of point, quanta,
background quanta, the longest gap any background process waited (and the
command that caused it), refused slices by reason, and the console stack's
high-water mark (the stack is painted at boot, and slices run on it below
whatever command reached the safe point). Every console command opens a
window; `sched last` says whether that window starved the background (judged
for windows of 100 ms or more; starved if nothing ran or a gap exceeded
250 ms).

## Consequences

- A client can now be run in the foreground against a live service
  (`run /bin/tick-client` gets its reply; with scheduling paused it times out,
  exactly as before V0.10).
- Busy console commands no longer starve the background: measured
  `max_gap_ms` 50 during `busy`, 61 during a CPU-bound foreground program that
  never yields, 49 during `net poll` (leg `sched-always-on-bios`).
- The guarantee is bounded and cooperative, not preemptive: inside an R10
  region the background waits for the region to end. Those regions are
  bounded and listed in `docs/KNOWN_LIMITATIONS.md`.
- A background process's effective quantum is 10–20 ms of Ring 3 time;
  syscall time is not charged to it.
- The upgrade path to a preemptible kernel is an executive task on top of S2
  (counted locks — a held lock is already visible) and S3 (every address
  space shares the task-stack window), plus per-task transition state.
