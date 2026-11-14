# Proposed changes — NOT applied

Written 2026-11-14 during the Linux run recorded in
`docs/LOCAL_LINUX_RUN.md`. That run executed every step of
`.github/workflows/ci.yml` for the first time since V1.0 (v1.0.0 was released
without a CI run) and three QEMU legs failed. Each failure has a root cause
below.

The fixes are kept here as patch files and are **not applied to the tree**:
they change the CI workflow, a test assertion, or what the kernel reports and
audits on a security path, and that is the owner's decision, not an agent's.
The session's permission layer refused to let the agent commit them for the
same reason. The tree on this branch is the v1.0.0 code with documentation
and one new script only.

To apply one after review: `git apply docs/proposed/<file>.patch`, run the
gate, commit.

## 1. `fault-contain-uefi` cannot start in CI — patch 0001

**Failure.** `qemu-runner` panics before starting QEMU:
`--uefi requires --firmware or OVMF_CODE env var`.

**Root cause.** The workflow step passes `--uefi` without `--firmware`. Every
other UEFI leg in the workflow passes
`--firmware /usr/share/OVMF/OVMF_CODE_4M.fd`. `scripts/test.ps1` does not need
the flag because `scripts/env.ps1` sets `OVMF_CODE`; the workflow sets no such
variable. The leg was added for V1.0 and has never run in CI.

**Patch.** One line: add the `--firmware` flag to that step. No assertion
changes; `scripts/check-ci-parity.py` still reports `CI-PARITY: OK 74 legs`.

**Verified.** With the patch the leg passes on Linux (QEMU 8.2.2, OVMF
2024.02).

**Consequence if not applied.** The first GitHub CI run of v1.0 fails at this
step.

## 2. `ai-retry-bios` fails about every other run — patch 0002

**Failure.** The leg forbids `rollback_kill_refused`, and the kernel prints
`rollback_kill_refused id=2 target=flapd pid=16 reason=not_an_instance`.
Reproduced on an idle host: 3 failures in 5 isolated runs. This is the leg the
V1.0 checkpoint recorded as flaking "under host load"; load is not the cause.

**Root cause.** A race in the rollback of `retry-service` (ACT11-002,
`kernel/src/ai.rs`, `rollback_retry`). `flapd` exits by itself as soon as it
runs. init's `stop` acknowledgement names the instance to end (pid 16). If
that process is scheduled and exits before the kernel reads the
acknowledgement, `is_instance_of` finds it terminal and returns false, and
the kernel reports — and audits as a denial — a refused kill of a process
that was a genuine instance and is simply already gone. If the process has
not run yet, the kernel kills it and the leg passes. Which happens depends on
scheduling order, not on anything the test controls. The serial log of a
failing run shows the order: `mailbox ack seq=3 ok=true pid=16`,
`user_enter pid=16`, `FLAPD-START`, `user_exit pid=16 code=0`, then
`rollback_kill_refused`.

**Patch.** The decision moves into a pure function,
`kernel_core::initctl::rollback_kill(present, terminal, instance)`, with a
host test over all eight inputs:

| Process named by init | Result |
|---|---|
| live, an instance of the service | killed (unchanged) |
| already ended, or no longer in the table | `rollback_instance_gone` — nothing to kill, no denial audited (new) |
| live, not an instance | `rollback_kill_refused ... reason=not_an_instance`, denial audited (unchanged) |

The kill stays reachable only for a live instance, so init gains no
authority. The leg's assertions are unchanged.

**What the owner should weigh.** This changes what the kernel prints and
audits on a security path: a refusal line and a denied audit record no longer
appear when the named process has ended. A compromised init naming a dead or
non-existent pid is now reported as `rollback_instance_gone` rather than as a
refusal; nothing is killed either way. If the refusal should stay for a pid
that was never an instance, the function has the inputs to say so
(`terminal && !instance`).

**Verified.** With the patch: `ai-retry-bios` 6 passes in 6 runs on Linux,
five through the gone path and one through the kill path; `cargo fmt`,
both clippy invocations, the `initctl` host tests and the unsafe inventory
pass. Not run: the recorded mutation controls for ACT11-002 (the scripts are
in `E:\claude-tmp\itisyou-audit\v011\`, not in the repository), and the
Windows gate.

**Consequence if not applied.** CI is red about every other run on this leg.
The published v1.0.0 images carry the race; it is recorded in
`docs/KNOWN_LIMITATIONS.md`.

## 3. `fault-contain-bios` cannot pass on QEMU 8.2 — no patch

**Failure.** `out-of-order: FAULTPROBE-ARMED mode=step`: the `int1` mode was
expected to raise `#DB` (vector 1) and raised `#UD` (vector 6):
`exception=invalid_opcode cs_rpl=3`, `Fault { vector: 6 }`. The program was
contained either way and the system went on.

**Root cause.** The emulator, not the kernel. `int1` is the one-byte opcode
`0xF1` (ICEBP). In QEMU 8.2.2, `target/i386/tcg/translate.c` handles `0xf1`
only inside `#ifdef WANT_ICEBP`, which is not defined, so the opcode is
undefined and raises `#UD`. QEMU 10.0's decoder has an `INT1` entry for
`0xF1` and raises `#DB`. (Read from the QEMU sources at tags `v8.2.2` and
`v10.0.0`.) The leg was written against QEMU 11.1.0 on Windows. Ubuntu 24.04
— this host and GitHub's `ubuntu-24.04` runner, which installs the same
`qemu-system-x86` package — has 8.2.2.

**Why there is no patch.** The two ways to make the leg pass are both the
owner's call:

- *Change the environment:* have the workflow install a QEMU that implements
  `int1` (9.x or later; the exact first version was not determined) instead
  of the distribution's 8.2.2. The assertions stay as they are. This also
  brings CI closer to the QEMU the project develops against.
- *Change the assertion:* assert `int1` as contained without naming the
  vector, as the same leg already does for `stack`
  (`Fault { vector: `). The `#DB` gate would still be asserted exactly by
  `step` and `#UD` by `sse`. This loosens one assertion, which the project's
  rules do not allow an agent to do.

**Consequence if nothing is done.** The first GitHub CI run of v1.0 fails at
this step on `ubuntu-24.04`.
