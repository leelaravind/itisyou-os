# ADR-0014: Long-running Ring 3 services

## Status

Accepted and verified for V0.8 (`services-bg-bios`). **Superseded for boot
services by ADR-0023 (V0.10):** the persistent services are started and
supervised by `/sbin/init` from `/etc/init.conf`, not by a static kernel
table. The on-demand supervisor (`svc`) described here is unchanged.

## Context

V0.7 could supervise services, but only run-to-completion ones: `run_supervised`
started a service, scheduled the process table until every member reached a
terminal state, and reported. That is enough to prove dependency ordering and a
bounded restart policy, and not enough to call the result a system — nothing
survived the command that started it, so a client could never talk to a service
that was still running, because it never was.

Making a daemon persistent raises three questions the V0.7 model never had to
answer: who gives it CPU while the operator is at the prompt, what "failure"
means for a process that is not supposed to finish, and how the daemon paces
its own periodic work.

## Decision

**Idle time belongs to the services.** The shell's input loop previously spun
on `core::hint::spin_loop()` waiting for a serial byte. It now spends that time
in `services::pump()`, a bounded scheduling slice (one 10 ms tick). The shell is
waiting on a human either way; the difference is that the CPU now does work.
Keystroke latency stays bounded because the slice is bounded in real time, and
no service can extend it by refusing to yield — preemption already ends the
quantum.

**A daemon's clean exit is a fault.** `ServiceDef::long_running` marks services
that are expected to outlive every command. For those, `pump` inverts the exit
verdict: returning with code 0 is treated exactly like a crash, because a
service that has stopped serving has failed regardless of how politely it
stopped. Both then take the same bounded restart path
(`service::RESTART_LIMIT`, aliased rather than restated so the on-demand and
background supervisors can never drift apart), so a broken daemon produces at
most three restarts and then a `Failed` row — never a restart storm.

**Services pace themselves in ticks, not scheduling passes.** `SYS_UPTIME`
returns the monotonic tick counter. A daemon that instead counts its own loop
iterations is measuring system load, not elapsed time: `tickd` running a
pass-based heartbeat emitted two lines while the machine was busy and 240 while
it was idle, drowning the serial console it shares with the shell. The syscall
conveys no authority — a free-running counter, with no wall clock and nothing
about any other process — so, like `getpid`, it is ungated.

**`bg` co-schedules, `run` does not.** `run` executes a program start to finish
with nothing else on the CPU, which makes an IPC round trip with a live service
impossible by construction: the service is not running while its client is.
`bg` admits the program and pumps until it terminates, bounded by a 30 s
ceiling, so client and daemon are runnable in the same period.

## Consequences

Background services keep their own rows in the status table, so `run_supervised`
clears only the on-demand rows and counts only on-demand services in its report
— folding a failed background daemon into that report produced `started=3
failed=2`, which is arithmetic nonsense.

Services are still ordinary Ring 3 processes holding exactly the capabilities
they declare (`tickd` IPC-only, `flapd` none). There is still no privileged
daemon, and persistence buys a service no authority.

The selftest kernel deliberately does not start them: background daemons
scheduled in the middle of the scheduler and userspace tests would make a
deterministic process table impossible.

## Evidence

`artifacts/qemu/services-bg-bios.result.json`. `tickd` heartbeats carry real
kernel ticks at a fixed 2 s cadence; two `bg` clients complete IPC round trips
against the same instance (its own counter reaches `n=2`, which a restarted
service could not print); `flapd` is restarted three times and then marked
`Failed`; the on-demand supervisor still reports `started=3 done=2 failed=1
restarts=3` alongside them; and the shell still accepts commands afterwards.
