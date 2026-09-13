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
9. **Device-model suite** (V0.6, inside the selftest kernel): PCI enumeration
   into `Device` records, NVMe MMIO BAR sizing, capability-list walking (NVMe
   MSI-X/PCIe/PM), the non-destructiveness of the BAR probe, and Ring 3 device
   access via the `devinfo` syscall (`/bin/lsdev`). Adversarial capability-list
   (circular / self-loop / out-of-range) and malformed-descriptor cases are
   host-tested in `kernel_core`.
10. **AC97 audio leg** (V0.6, `audio-bios`): attach an AC97 controller with
    QEMU's `wav` backend; the OS binds the driver (B190), `beep` DMAs a tone
    through the PCM-Out bus master, and the runner asserts the init + full-
    consumption markers **and** that the captured WAV holds non-silent PCM —
    the samples must reach the output, not just initialize.
11. **USB (UHCI) + HID leg** (V0.6, `usb-hid-bios`): attach a UHCI controller +
    USB HID keyboard; the OS enumerates it over control transfers (device +
    config descriptors, address/config/protocol) and the graphical desktop
    reads HID input off the interrupt endpoint through the **unified** input
    queue; an injected keypress arrives tagged `src=usb` — real USB input and
    input-source unification.
12. **Platform security suite** (V0.7, selftest + `platform-bios` leg): a
    zero-capability probe attempts all 7 privileged syscall classes and must
    see every one denied (`SANDBOX-DENIED-OK`); FS-sandbox probe (out-of-
    prefix, foreign app dir, `..` traversal, root escape all denied; in-
    sandbox miss stays NOENT); delegation probe (child requesting caps its
    parent lacks finds them denied, delegated ones work); denial auditing.
13. **Service supervision suite** (V0.7): deterministic dependency-ordered
    startup, a service serving a dependent client over IPC (3 ping→pong),
    crash containment with the bounded restart policy (exactly 3 restarts →
    Failed), no leaked processes, cycle detection.
14. **Package/update suite** (V0.7, selftest + two-boot legs): verified
    install; corrupted-payload and hostile-manifest packages refused with
    the store untouched; manifest-only-capability launches; atomic update;
    atomic rollback (previous version reactivates, evidence kept);
    interrupted update staged on boot 1 survives a REAL reboot as an orphan
    that never activates, and boot 2's recovery scan removes it
    (`update-interrupt`/`update-recovery` legs on one persistent disk).
15. **Long-running services suite** (V0.8, `services-bg-bios` leg): `tickd`
    and `flapd` start at boot with exactly their declared capabilities
    (`caps=0x2` / `caps=0x0`) and are supervised for the life of the system.
    The leg asserts all four properties that separate a daemon from V0.7's
    run-to-completion tasks: still ALIVE later in the boot (heartbeats stamped
    with real kernel ticks, so the cadence is load-independent); still
    SERVING, not merely resident (two `bg` clients complete IPC round trips
    against the same instance — `TICKD-SERVED n=2`, a number a restarted
    service could not print); a daemon's clean exit treated as a FAULT and
    restarted under the bounded policy, then `bg_failed ... restarts=3` rather
    than a restart storm; and the system staying healthy throughout (the
    on-demand supervisor still reports `started=3 done=2 failed=1 restarts=3`,
    the `svc` table shows background and on-demand state together, and the
    shell still accepts commands afterwards).
16. **Networking suite** (V0.8, `net-bios`): the guest's NIC is wired to the
    runner's own host-side Ethernet peer over a `dgram` netdev — no slirp, no
    host resolver, nothing outside the machine — and that peer is an
    INDEPENDENT byte-level implementation, so a bug in the guest's codec cannot
    cancel itself out against the same code. Client paths (ARP, ICMP echo, a
    Ring 3 UDP round trip, DNS), responder paths (answering the peer's ARP
    request and ping), the capability gate (an app with `network` succeeds; one
    with none is refused every syscall), and refusal: five hostile frames
    counted as refused and answered by nothing.
17. **Userspace filesystem writes** (V0.8, `fs-write-bios` + `fs-write-persist`):
    create, atomic overwrite to a different length, list, delete, and four
    refusals each for a different reason; the WRITE right proved distinct from
    READ; the sandbox proved distinct from the capability; and a second boot on
    the same disk reading back what Ring 3 wrote.
18. **Interrupt modernization** (V0.8, `irq-bios`): a delivered APIC timer
    interrupt, a delivered MSI-X raised by a real NVMe block read, an I/O APIC
    register round-trip left masked, and the PIC's own counters proving it is
    still the path doing the work.
19. **xHCI** (V0.8, `xhci-hid-bios`): reset, command/event rings, Enable Slot,
    Address Device, control transfers, Configure Endpoint,
    SET_CONFIGURATION/SET_PROTOCOL, and a real injected keypress arriving over
    an interrupt transfer tagged `src=xhci`.
20. **Persistent audit** (V0.8, three boots): saved, recovered and verified by a
    fresh guest with the same chain head, then reported TAMPERED once the
    stored trail is overwritten.
21. **Hardening** (V0.8, `harden-bios`): SMEP/SMAP/UMIP on, a W^X segment
    refused at load, `sgdt` from Ring 3 a contained #GP, the stack guard exactly
    16 pages down, and a Ring 3 read of a kernel address refused. Two
    assertions are `--forbid`: a refusal prints no line, so the only way to
    state it is that the success marker never appeared.
22. **Negative cases** (grown alongside subsystems): intentional panic,
    allocator exhaustion, malformed inputs, timeout classification.

The harness gained `--audio`/`--audio-out` (AC97 → WAV capture, with a non-
silence assertion), `--usb` (UHCI + USB HID), `--xhci` (an xHCI controller with
its own HID keyboard), `--net` (an e1000 wired to the runner's own Ethernet
peer), `--net-user` (an e1000 on QEMU's user-mode network, an independent IPv4
implementation with its own gateway, DHCP server and DNS forwarder; added in
V0.8.1), and `--forbid` (a substring that must NOT appear) alongside the
serial + monitor channels. Every leg now runs on a CPU advertising `+smep,+smap,+umip`,
so the whole matrix passing is itself evidence that supervisor-mode protection
did not break the kernel's own access to user memory.

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
scripts\test.ps1     # host unit tests + the full 24-leg QEMU matrix
scripts\verify.ps1   # canonical full gate (adds doctor, fmt, both clippy gates, website, secret scan)
```

## Release-consistency and website verification

- `website/scripts/check-consistency.mjs` runs inside `npm run verify` (so in
  `scripts/verify.ps1` and in every CI run). It fails the build when
  `status/current.json` names a different version from the Cargo workspace
  (the kernel's own `CARGO_PKG_VERSION`), when a requirement row uses a state
  outside the vocabulary, or when a `verified` status carries unstarted rows.
  `node website/scripts/check-consistency.mjs --release` is the stricter form
  run before a version tag: no non-terminal row at all. It was written against
  the `v0.8.0` tree, where it reports exactly the three defects that release
  shipped with.
- `website/scripts/browser-verify.mjs --base <url> --expect-text <text>`
  drives a headless Chromium over the DevTools protocol (no dependencies): every
  route at 1440 px and 390 px, console errors, uncaught exceptions, browser log
  errors (CSP violations, blocked resources), failed or ≥ 400 sub-requests,
  horizontal overflow, `<main>`/`lang`/`alt` checks, broken images, every
  internal link, the 404 page, and the security headers. Screenshots and a JSON
  report go to `$ITISYOU_SCRATCH\browser-verify`. It is run against staging and
  then production for every release; HTTP status alone is not verification.

## Rules

- No skipped/disabled tests to obtain green status; a bug fix lands with a
  regression test that demonstrates the failure mode (plan §12.4).
- A test "passing" is only claimed from an actual run's evidence, never from
  the existence of test code.
