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
4. **Userspace suite** (V0.2/V0.3, inside the selftest kernel + shell leg):
   Ring 3 execution of `/bin/init` with the full syscall ABI (its RING3-*
   output lines are `--require`d by the harness), malformed-ELF and W^X
   rejection fixtures, #GP/#PF containment programs, per-process address-space
   isolation + leak proofs, concurrent spawn/wait/IPC (parent + children).
5. **Preemption suite** (V0.4): two non-yielding CPU-bound processes finish
   with registers + address spaces intact; an infinite spinner cannot
   monopolize (co-scheduled finite process still completes); cooperative
   scheduling coexists; no frame leaks / lost processes.
6. **Storage/filesystem suite** (V0.3/V0.4): PCI enumeration, RAM-disk and
   NVMe block I/O, NVMe write→flush→read round-trip, ITFS format/create/
   read/list, strict metadata validation, crash-consistency (torn
   superblock recovery), and a **two-boot reboot-persistence** test (UEFI)
   proving a written file survives a full QEMU reboot on the same disposable
   disk.
7. **Graphics/compositor/GUI suite** (V0.5, inside the selftest kernel):
   framebuffer availability + sane geometry, fill/glyph pixel read-backs, a
   deterministic CRC region hash, a composited window verified on screen, a
   **Ring 3 process's window colour read back off the composited screen**
   (proving userspace rendered), and adversarial rejections — cross-owner
   draw, out-of-bounds rectangle, bad window size — plus the keyboard/mouse
   decoders (including malformed mouse packets).
8. **Desktop + PS/2 input suite** (V0.5, `desktop-input-bios`): boots the
   interactive kernel, enters the graphical desktop, then injects **real**
   keyboard (`sendkey`) and mouse (`mouse_move`/`mouse_button`) events through
   the **QEMU HMP monitor** into the emulated PS/2 devices. The kernel's
   IRQ1/IRQ12 handlers must observe them (`[ITISYOU:INPUT]` markers), the
   compositor re-renders and captures a framebuffer `screendump` (the OS draws
   the desktop — no host UI), and `DESKTOP-INPUT-VERIFIED` gates success. This
   proves the full graphics + input path inside QEMU.
9. **Negative cases** (grown alongside subsystems): intentional panic,
   allocator exhaustion, malformed inputs, timeout classification.

The harness supports two guest channels: a **serial** line (stage/test markers,
shell stdin) and an optional **HMP monitor** (`--monitor`, `--inject-after`,
`--monitor-cmd`) used to inject PS/2 input and capture screendumps once a gate
marker appears.

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
