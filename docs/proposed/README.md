# Open decision — `fault-contain-bios` on QEMU 8.2

Written 2026-11-14 during the Linux run recorded in
`docs/LOCAL_LINUX_RUN.md`. That run executed every step of
`.github/workflows/ci.yml` for the first time since V1.0 and three QEMU legs
failed. Two are fixed on this branch (`fault-contain-uefi`: commit
"ci: fault-contain-uefi names its firmware"; `ai-retry-bios`: commit
"v1.0: an instance that already ended is not a refused rollback kill
(ACT11-002)"). This file is about the third, which is **not fixed** and
needs the owner.

## The failure

`out-of-order: FAULTPROBE-ARMED mode=step`. The `int1` mode of
`/bin/fault-probe` was expected to raise `#DB` (vector 1) and raised `#UD`
(vector 6): `exception=invalid_opcode cs_rpl=3`, then
`run: /bin/fault-probe: Fault { vector: 6, addr: None }`. The program was
contained and the system went on; every other assertion of the leg held.

## Root cause

The emulator, not the kernel. `int1` is the one-byte opcode `0xF1` (ICEBP).
In QEMU 8.2.2, `target/i386/tcg/translate.c` handles `0xf1` only inside
`#ifdef WANT_ICEBP`, which is not defined, so the opcode is undefined and
raises `#UD`. From QEMU 9.1.0 the decoder (`decode-new.c.inc`) has an
`INT1` entry for `0xF1` and raises `#DB`. Read from the QEMU sources at each
release tag:

| QEMU | `0xF1` |
|---|---|
| 8.2.2, 9.0.0 | only under `#ifdef WANT_ICEBP` (undefined) in `translate.c` → `#UD` |
| 9.1.0, 9.2.0, 10.0.0 | `X86_OP_ENTRY0(INT1, svm(ICEBP))` in `decode-new.c.inc` → `#DB` |

So the leg needs **QEMU 9.1.0 or later**. This is read from source, not
observed: no QEMU newer than 8.2.2 was run in the session.

The leg was written against QEMU 11.1.0 on Windows. Ubuntu 24.04 — the host
of this run, and GitHub's `ubuntu-24.04` runner, which installs the same
`qemu-system-x86` package in the workflow's "Install QEMU + OVMF" step — has
8.2.2. **The leg therefore fails on every GitHub CI run as the workflow
stands.**

## Why it was not fixed here

There are two ways to make the leg pass, and neither could be done in the
session that found it:

1. **Change the environment, keep the assertion.** Have the workflow use
   QEMU 9.1.0 or later instead of the distribution's 8.2.2. Ubuntu
   24.04 has no newer QEMU package, so this means building QEMU in the
   workflow (several minutes per run, cacheable) or another runner image. It
   was not written because it could not be verified: building a newer QEMU
   in the session, to show the leg passes on it unchanged, was refused by
   the session's permission layer. An unverified workflow change is not a
   fix. It would also bring CI closer to the QEMU the project develops
   against.
2. **Change the assertion.** Assert `int1` as contained without naming its
   vector, as the same leg already does for `stack`
   (`run: /bin/fault-probe: Fault { vector: `). The `#DB` gate would still
   be asserted exactly by `step`, and `#UD` by `sse`, so no IDT gate would
   lose its exact assertion. This loosens one assertion. The task rules
   ("never skip, weaken or delete a test") and the session's permission
   layer both forbid an agent from doing it; it needs the owner's decision
   and the same edit in `.github/workflows/ci.yml` and `scripts/test.ps1`
   (the parity check compares them).

## What the owner has to do

Choose 1 or 2, apply it, and see the step pass on GitHub's runner. Until
then a GitHub CI run of this tree is red at "QEMU V1.0 every CPU exception
from Ring 3 is contained (BIOS)" and green everywhere else that this
session could check.
