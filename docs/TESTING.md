# Testing — ITISYOU OS

## Test pyramid

1. **Host unit tests** (`cargo test -p kernel-core -p image-builder -p
   qemu-runner`) — pure logic: stage/marker contract, SHA-256 vectors, and
   (as subsystems land) memory-map normalization, allocator logic, path
   handling, shell parsing. Fast, deterministic, no QEMU.
2. **QEMU boot smoke** — boots the interactive kernel (BIOS and UEFI),
   asserts stages B010/B020/B030 appear within the timeout.
3. **QEMU selftest suite** — boots `itisyou-kernel-selftest`, which runs
   in-kernel checks, emits `[ITISYOU:TEST]` events and a
   `[ITISYOU:SELFTEST] pass=N fail=M` summary, then exits QEMU through
   `isa-debug-exit`. The harness requires: all expected stages, `fail=0`,
   `pass>0`, **and** QEMU exit code 33. A hang, panic, or missing summary
   can never pass.
4. **Userspace suite** (V0.2, inside the selftest kernel + shell leg):
   Ring 3 execution of `/bin/init` with the full syscall ABI (its RING3-*
   output lines are `--require`d by the harness), malformed-ELF and W^X
   rejection fixtures, #GP/#PF containment programs, kernel-alive and
   reload-after-teardown checks.
5. **Negative cases** (grown alongside subsystems): intentional panic,
   allocator exhaustion, malformed inputs, timeout classification.

## Failure classification (tools/qemu-runner)

| Outcome | Meaning |
|---|---|
| `success` | expected markers + clean exit (selftest: exit 33, fail=0) |
| `selftest-failed` | in-kernel test failure (or exit 35) |
| `panic` | `[ITISYOU:PANIC]` observed |
| `timeout` | deadline exceeded; QEMU killed; serial tail preserved |
| `missing-markers` | exited without required stage/summary markers |
| `launch-failure` | QEMU process could not start |
| `unexpected-exit` | exit status outside the contract (e.g. reset loop) |

Evidence per run: `artifacts/qemu/<label>.serial.log` +
`<label>.result.json` (outcome, exit code, stages seen/missing, test events,
duration, full command line).

## Commands

```powershell
scripts\test.ps1     # unit tests + 4-way QEMU matrix
scripts\verify.ps1   # canonical full gate (adds fmt, clippy, doctor, secret scan, website)
```

## Rules

- No skipped/disabled tests to obtain green status; a bug fix lands with a
  regression test that demonstrates the failure mode (plan §12.4).
- A test "passing" is only claimed from an actual run's evidence, never from
  the existence of test code.
