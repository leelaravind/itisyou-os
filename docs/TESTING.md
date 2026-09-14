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
    create, atomic overwrite to a different length, list, delete, and five
    refusals each for a different reason; the WRITE right proved distinct from
    READ; the sandbox proved distinct from the capability; and a second boot on
    the same disk reading back what Ring 3 wrote. V0.11: a name with a line
    break in it is refused and no forged `[ITISYOU:AUDIT]` line appears
    (AUDIT11-002); reading, writing and deleting the audit trail and a real
    installed package's files are refused and audited, and `pkg list` shows
    the package still active (SEC11-001); contents a program wrote with a line
    break and a marker prefix in them come back from `store cat` as one line
    with the prefix rewritten.
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
    stored trail is overwritten (since V0.11 by the read-only `audit verify`:
    `trail_checked status=TAMPERED reason=chain`). V0.11 adds three more boots
    (`audit-window-write`/`-continue`/`-verify`, AUDIT11-001): more records
    than the stored window holds, so the save must move `base` off genesis;
    a second boot that recovers, adds records, checks mid-boot and saves; and
    a third that must find that trail verified. Four more boots on the
    tamper disk (`audit-tamper-boot`/`-save`/`-stays`/`-cleared`,
    AUDIT11-003) show boot recovery reporting a forgery, a save that does
    not launder a tampered trail, the next boot still reporting it, and a
    fresh trail verifying once the operator has removed it. The anchor test (V0.9, four
    boots on one disk: `audit-anchor-save`/`-match`/`-forge`/`-detect`)
    anchors the saved head with the runner's witness over QEMU's user-mode
    network, matches it on a fresh boot, writes a valid empty trail, and then
    requires boot 4's recovery to call the forgery verified (the unkeyed
    chain's weakness) while the anchor check reports `MISMATCH`; since V0.11
    `audit verify` in the forging boot already reports `reason=replaced
    records=0` while the system is still up.
21. **Hardening** (V0.8, `harden-bios`): SMEP/SMAP/UMIP on, a W^X segment
    refused at load, `sgdt` from Ring 3 a contained #GP, the stack guard exactly
    16 pages down, and a Ring 3 read of a kernel address refused. Two
    assertions are `--forbid`: a refusal prints no line, so the only way to
    state it is that the success marker never appeared.
22. **ITFS space reclamation** (V0.10, `fs-reclaim-churn` + `fs-reclaim-persist`):
    an 8 KiB disk (14 data blocks) takes 21 one-block writes through the real
    NVMe path — more than the V0.9 bump allocator could ever place — with every
    write succeeding; `store df` prints `[ITISYOU:FS] reclaim …` with exact
    used/free/pinned/largest-run numbers predicted by a host test, and a second
    boot on the same disk reads back the last revision and the same report.
23. **Always-on co-scheduling** (V0.10, `sched-always-on-bios` +
    `sched-pause-bios`, which replaced the early control leg
    `sched-always-on-negative`): background processes keep running while the
    console is busy. The runner's `@pause <ms>` send holds the console idle at
    its prompt (idle slices must be non-zero); `busy` with scheduling paused
    must starve the background and, resumed, must not (`sched last` judges each
    command's window: starved if nothing ran or any gap exceeded 250 ms); a
    service client run in the foreground must get its reply, a CPU-bound
    program that never yields must still leave the background a bounded gap,
    and programs under `run` must be able to wait for a child and sleep.
    `sched-pause-bios` pins down what pause stops: busy and idle points, never
    the console's own job. `sched` also reports `lock_skips` (must be
    0) and the console stack's painted high-water mark (`stack_margin_ok`).
24. **Process model** (V0.10, `proc-model-bios`): `/bin/proc-probe` checks the
    process tree from inside — a sibling may not collect another process's
    child (refused and audited, and the child is still its parent's to
    collect), `wait_nohang` on no child / a running child / "any", `sleep`
    duration and bound, and an orphan that the kernel reaps once it ends (a
    `--forbid` on its path in the final `ps` states that nothing is left).
    The same probe acts as a supervisor to check `svc_report`: every refusal
    the kernel can decide by itself (not its child, a reserved name, a row it
    does not own, `ready` from a non-init, no capability) and one accepted
    report about its own child.
25. **Userspace init** (V0.10, `init-bios`): init boots as pid 1 and starts
    tickd and flapd (pids 2 and 3) before the console — since V0.11 also
    inferd and flakyd (pids 4 and 5, `services=4`); `/sbin/init` checks the
    shipped `/etc/init.conf` and two broken fixtures (a dependency cycle, an
    unknown capability) with exact line and reason, is refused the file
    without the filesystem capability, and supervises a service from a test
    config through to `svc`; an orphan goes to init; `kill 1` ends init's
    tree and the kernel restarts it, and a client is then served by the new
    tickd.
26. **Ring 3 shell** (V0.10, `rsh-bios`): the kernel lends the console's
    input to `/bin/sh`; the leg's forbids prove which shell read each line —
    the kernel never saw the shell's lines, and the shell never saw the ones
    after `exit` — and a process that does not own the input is refused.
27. **Desktop focus and input routing** (V0.10, `desktop-focus-bios`): a
    persistent Ring 3 app on the live desktop; the QEMU monitor parks the
    cursor in the corner (in steps small enough that QEMU's PS/2 queue sends
    each move whole), moves it onto the app and clicks, types, clicks the
    desktop's window and types again; the forbid on `GUIECHO-KEY b` states
    that a key typed after the focus left the app never reached it.
28. **Kernel copies into user memory** (V0.10, `uaccess-bios`): a program with
    no capability points `args` and `cap_list` at its own read-only code and
    data; the kernel must refuse (`ERR_FAULT`) — before SEC10-001 it panicked.
29. **Diagnostic model** (V0.11, `ai-model-bios`, MODEL11-001): the build
    trains the model from `ai/scenarios.txt` and refuses a stale pin; at boot
    the kernel must print `[ITISYOU:AI] model sha256=<pin> bytes=276
    initramfs_match=true decode=ok`, with the pin read from
    `ai/diag.model.sha256` by the leg itself — the initramfs carries exactly
    the bytes the build trained and the kernel was compiled against. On CI the
    build trains on Linux, so the same pin there is the cross-host check. The
    accuracy figures and the codec's refusals are host tests
    (`kernel_core::model`).
30. **Approved view and terminal escaping** (V0.11, `ai-view-bios`,
    VIEW11-001 and OUT11-001): `sys_view` is refused to a program with no
    capabilities (`reason=no_handle`) and to a `run` without a caps list
    (`reason=scope_denied`: the legacy default does not carry it); granted by
    name, the record decodes with the host-tested codec and tells the truth —
    flapd failed after 3 restarts, tickd running, both reported by init,
    `paused=1` after `sched pause` — and a buffer one byte short gets nothing
    (`AIPROBE-VIEW-SMALL-OK`). In the same leg a program writes cursor-up,
    erase-line and a carriage return before `forged`; the log must show them
    escaped on one line (`AIPROBE-ANSI \x1b[2A\x1b[2K\x0dforged`).
31. **Ring 3 inference** (V0.11, `ai-infer-bios`, INFER11-001): init starts
    inferd as pid 4 (`caps=0x12`) with the shipped model; its `check` mode
    accepts that file and refuses each hostile fixture for its own reason
    (`bad_magic`, `bad_length`, `bad_dimensions`, `weight_out_of_range`).
    `/bin/ai-probe infer` asks inferd about the real view and recomputes the
    answer from the model file itself: `match=true model_match=true` for
    `service_failed`, and for `service_failed,scheduler_paused` with the
    scheduler paused (job-wait slices still run inferd); `match=false` and a
    timeout are forbidden.
32. **Diagnostic agent and runbooks** (V0.11, `ai-diagnose-bios`,
    AGENT11-001 and KB11-001): three situations the leg constructs, each
    with its EXACT condition set — the booted system (`service_failed`,
    `action=retry-service target=flapd`), the scheduler paused
    (`service_failed,scheduler_paused`, `action=resume-scheduler`) and a
    burst of denials from `sandbox-probe` run with no capabilities
    (`service_failed,denial_burst`, `action=none`) — every condition's
    runbook cited by id and digest, and the agent refused without the view
    capability. `conditions=none` is forbidden: the healthy case cannot occur
    on the shipped image (flapd fails on every boot) and is host-tested only.
33. **Proposals** (V0.11, `ai-propose-bios`, PROP11-001): ten adversarial
    `ai-probe` runs, each its own process — an unknown model, a view never
    served and a forged one, a stale view, a condition the model does not
    find, an action that does not apply, a marker smuggled in as the target,
    an unknown action, an action the condition may not propose, a second
    pending proposal. Every refusal reason must appear and no probe may be
    accepted (`AIPROBE-ACCEPTED` forbidden); the leg checks the set of
    reasons, not which run produced which. Without `propose` the capability
    gate refuses; with the agent's authority `svc_report`, `fs_write` and
    `spawn` are all refused; the agent's real proposal is filed, listed with
    its preview, and `deny` is final. `action_executed` is forbidden — this
    leg has no `approve`.
34. **Approval, execution, verification** (V0.11, `ai-act-bios`,
    ACT11-001): on a paused system a denied proposal cannot be approved and
    `busy 1000` shows the background still starved; an approved
    `resume-scheduler` executes, passes its verification (the `busy`
    measurement: other processes must progress at busy points) and a second
    `busy 1000` shows the background running; a proposal made moot by
    resuming by hand is refused `precondition_changed` and expired, never
    executed. The `action_executed` audit record carries
    `approved_by=console` and the model and view digests.
    `action_rolled_back` is forbidden here: the rollback of
    `resume-scheduler` is exercised only by that row's mutation control.
35. **retry-service through init** (V0.11, `ai-retry-bios`, ACT11-002):
    flakyd has failed at boot (`restarts=3`); after `busy 3000`, so that its
    3 s of guest uptime have passed, a proposal to retry it goes through
    init's mailbox (`post`, `INIT-RETRY`, `ack`), executes and verifies
    (`tripped=false running_same_pid=true`). The agent's proposal to retry
    flapd trips the watch when flapd fails again, init stops it, the kernel
    kills the instance init names, and `svc` shows `flapd  Failed { restarts:
    1 }` — a deterministic rollback, not three more restarts. A non-init
    caller of `init_ctl` is refused `not_init`; `INIT-REAPED-ORPHAN` and
    `action_failed` are forbidden.
36. **Negative cases** (grown alongside subsystems): intentional panic,
    allocator exhaustion, malformed inputs, timeout classification.

The harness gained `--audio`/`--audio-out` (AC97 → WAV capture, with a non-
silence assertion), `--usb` (UHCI + USB HID), `--xhci` (an xHCI controller with
its own HID keyboard), `--net` (an e1000 wired to the runner's own Ethernet
peer), `--net-user` (an e1000 on QEMU's user-mode network, an independent IPv4
implementation with its own gateway, DHCP server and DNS forwarder; added in
V0.8.1), `@pause <ms>` in a `--send` list (hold the next command back so
the console sits idle; V0.10), and `--forbid` (a substring that must NOT
appear) alongside the
serial + monitor channels. `@pause` waits in the host's wall-clock time and
only holds typing back; a wait in GUEST time is the kernel console's own
`busy <ms>` (1–10000 ms of TSC time calibrated against the PIT, running
background slices at busy points and reporting whether other processes
progressed). Guest time runs slower than the wall clock under emulation, so
a leg that needs the guest to age uses `busy` — `ai-retry-bios` runs
`busy 3000` because flakyd counts guest uptime. Every leg now runs on a CPU
advertising `+smep,+smap,+umip`,
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
scripts\test.ps1     # host unit tests + the full QEMU matrix (64 legs in V0.11 development; 50 at v0.10.0, 36 at v0.9.0)
scripts\verify.ps1   # canonical full gate (adds doctor, fmt, both clippy gates, website, secret scan)
```

CI (`.github/workflows/ci.yml`) runs the same 64 legs, by label, with longer
timeouts; the ten legs V0.11 added (`audit-window-*` and the seven `ai-*`)
send and assert exactly what `scripts/test.ps1` does.

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
- CI job `reproducibility` builds every image from two clean checkouts in two
  different directories and fails if any SHA-256 differs; image-builder
  normalizes the two sources of non-determinism found so far (random GPT GUIDs,
  the embedded UEFI loader's link time and CodeView GUID).
- `scripts/release-boot-test.ps1 -Dir <dir> -Version <v>` boots the exact
  release files (UEFI + BIOS console, two-boot persistence, user-mode network)
  and reports their digests afterwards, which must be unchanged.

## Mutation controls (V0.11)

A leg that passes says nothing about a fix unless it fails without it. Every
V0.11 row in `docs/REQUIREMENTS.md` records at least one such control
(KB11-001's and MODEL11-001's are build-time: a runbook removed, or a
scenario bound moved, fails the kernel build). The QEMU controls were run
one at a time, the same way: one fix or
check is disabled by an exact source edit (whose anchor must match exactly
once, or the control is not run), the image is rebuilt, the legs that must
catch it are run, and the result is recorded — which required markers went
missing or which forbidden one appeared, with the serial log kept. The
source is then restored from a byte copy whatever happened, its SHA-256
compared with the original's, and its timestamp bumped so that cargo
rebuilds from the restored file instead of reusing the mutated build.
Controls on host logic follow the same pattern against
`cargo test -p kernel-core`; MODEL11-001's is a data change instead (one
scenario bound moved by 1 fails both the pin test and the kernel build).
What each control disabled and how the leg failed is in the row's evidence
(for example PROP11-001: without the recomputation, the false diagnosis is
refused only as `not_applicable`); the scripts that apply and restore the
edits are local tooling, not part of `scripts/`.

Some controls are experiments rather than removals: to show a check holds
against a component that misbehaves, the component is changed instead —
for ACT11-002 a deliberately compromised `/sbin/init` that names another
service's pid in its `stop` acknowledgement, or never acknowledges a
`retry` — and the kernel run both with and without its check.

`--require-order` (V0.11) lets a leg tie one line to another: the listed
substrings must appear in that order, each on a later line than the
previous one (greedy earliest match, host-tested in `qemu-runner`).
`ai-propose-bios` uses it to show each adversarial run was refused for its
own reason; with two runs swapped the leg fails `out-of-order:`, though
every reason still appears somewhere.

The V0.11 adversarial review (six reviewers over the V0.11 diff, each
finding verified independently) found two defects in v0.10.0's console
evidence — OUT11-002, SEC11-002 — and AUDIT11-003; each fix has a leg and a
control like the rest.

## Rules

- No skipped/disabled tests to obtain green status; a bug fix lands with a
  regression test that demonstrates the failure mode (plan §12.4).
- A test "passing" is only claimed from an actual run's evidence, never from
  the existence of test code.
