# Development story — ITISYOU OS

A continuously updated engineering log: decisions, failures, root causes,
fixes, and verification evidence. Newest entries at the bottom. No secrets.

---

## 2026-09-02 · Session 1 — Foundation

### 02:20 — Governance intake and discovery

- Read `AGENT_OPERATING_RULES.md` and the end-to-end implementation plan;
  both adopted as binding. Requirement matrix created
  (`docs/REQUIREMENTS.md`).
- Discovery results: GitHub CLI authenticated (`leelaravind`); no existing
  `itisyou-os` repo → will create private. No wrangler/Cloudflare token on
  the machine yet (deployment auth to be resolved at the deployment phase;
  claude.ai Cloudflare MCP integration exists as a fallback for read-only
  account inspection). Stitch design ZIP found in `E:\Project\os`, preserved
  unchanged in `design/`, and extracted to `design/stitch/` — 13 screens +
  `DESIGN.md` design-system spec.
- Storage rules applied: Rust toolchain reinstalled to
  `E:\toolchains\{rustup,cargo}` (nightly-2026-09-01 + `x86_64-unknown-none`,
  rust-src, clippy, rustfmt, llvm-tools); QEMU 11.1.0 extracted to
  `E:\tools\qemu` **without elevation** (7-Zip via `msiexec /a` administrative
  extraction, then NSIS payload extraction) because the interactive installer
  would have required a UAC prompt the user isn't present to approve.
  Scratch: `G:\claude-tmp`.

### 02:30 — Kernel skeleton and test harness authored

- Workspace: `kernel/` (lib + `itisyou-kernel` interactive bin +
  `itisyou-kernel-selftest` bin), `crates/kernel-core` (host-testable
  stage/marker contract), `tools/image-builder` (bindeps artifact
  dependencies → pure-Rust BIOS/UEFI images + SHA-256 manifest),
  `tools/qemu-runner` (deterministic boot assertion harness: timeout, panic,
  marker, selftest and exit-code classification; JSON evidence).
- Failure: first build failed — this nightly's cargo rejects
  `cargo-features = ["bindeps"]` in the manifest (`error: unknown Cargo.toml
  feature 'bindeps'`). Root cause: artifact dependencies must now be enabled
  via `[unstable] bindeps = true` in `.cargo/config.toml`. Fixed there;
  `per-package-target` remains a manifest feature. Rebuild launched.

### 02:50 — UEFI bootloader stage blocked upstream; BIOS-first fallback

- Failure: `bootloader v0.11.17` build script died compiling its UEFI stage:
  `rust-lld: error: undefined symbol: wcslen` (referenced by the pinned
  `uefi` crate's `FileInfo::from_uefi`). BIOS stages built fine.
- Root cause: upstream regression between recent nightlies and the
  bootloader's pinned UEFI dependencies — tracked as
  rust-osdev/bootloader#579 (open since 2026-08-10, no fix; #578 closed as
  duplicate). Not caused by project code.
- Decision: BIOS-only images for now (image-builder `uefi` cargo feature
  gates the UEFI path, default off; scripts/CI print explicit SKIPPED lines
  — no silent scope cut). QEMU/SeaBIOS remains a fully valid V0.1 execution
  environment. In parallel, probing nightly-2026-05-01 to see if the UEFI
  stage links there; if yes the workspace pin moves back and UEFI returns.
- Also fixed: bootloader's build script wrote its `cargo install` temp dir
  to `C:\Users\...\Temp` — `scripts/env.ps1` now routes `TEMP`/`TMP` to
  `G:\claude-tmp\tmp` for all project commands (storage rules §5).

### 02:55 — First verified boot; UEFI recovered via toolchain pin

- **B010–B030 verified in QEMU (BIOS)**: `selftest-bios` outcome=Success,
  exit 33, selftest pass=2 fail=0. Harness gap found honestly: the
  interactive kernel halts by design, so the smoke run timed out — added
  `--exit-after-markers` (success once all expected stages observed).
- **UEFI restored**: probing found `bootloader-x86_64-uefi@0.11.17` links
  cleanly on nightly-2026-08-01 (the wcslen regression landed 2026-08-01 →
  2026-08-10; older nightlies fail differently — the crate's lockfile needs
  newer `Step` trait methods). Workspace re-pinned to nightly-2026-08-01,
  `uefi` back in default features, UEFI legs restored in scripts + CI.
- Foundation checkpoint pushed: private repo
  `github.com/leelaravind/itisyou-os`, commit `caae006`.

### 03:10 — Phase 2 subsystems: three root-caused failures

Implemented B040–B150: PMM (bitmap over validated map), paging
(OffsetPageTable wrapper, W^X policy), heap (linked_list_allocator, 1 MiB),
GDT/TSS/IDT + exceptions with double-fault IST, PIC+PIT timer @100 Hz,
cooperative round-robin scheduler with real context switch (naked fn),
VFS + build-time-packed ustar initramfs, serial shell (13 commands), plus
30+ in-kernel selftests and a panictest binary. Failures found by the
harness, root causes from serial evidence:

1. **Triple fault before B040 (both firmwares)**: the 128 KiB
   `PhysicalMemoryManager` was constructed on the 128 KiB kernel boot stack
   before being moved into its static → stack overflow → page fault with no
   IDT yet → triple fault. Fix: const-construct the PMM inside the static
   (`.bss`) and initialize in place. Regression protection: the comment on
   the static + selftests boot the full init path every CI run.
2. **UEFI-only panic `TooManyRegions`**: OVMF hands over 104 memory regions
   (BIOS: 10); the normalization bound was 64. Fix: bound raised to 256.
   The panic itself was the designed invariant behavior — evidence, not a
   silent hang.
3. **Shell test stalled after ~16 input bytes**: QEMU's Windows stdio
   chardev stops feeding redirected stdin once the guest UART's 16-byte RX
   FIFO fills. Fix: harness serial transport moved to TCP (runner listens,
   QEMU connects with `-serial tcp:...`); works identically on Windows and
   Linux CI. Also: panic marker made single-line (PanicInfo's Display splits
   message/location across lines, breaking the one-line marker contract).
4. **Runner hang on Windows**: a TCP stream accepted from a nonblocking
   listener inherits nonblocking mode on Windows → the reader thread died
   instantly on WouldBlock → the loop exited without killing QEMU →
   `child.wait()` blocked forever on the halting interactive kernel. Fix:
   force the accepted stream back to blocking + a bounded grace-then-kill
   before reaping. Full matrix green afterwards: selftest pass=27 fail=0 on
   BIOS **and** UEFI, shell interaction through B150, panic negative path.

### 04:10 — Deployment and a Linux-only CI failure

- Cloudflare discovery: claude.ai MCP integration confirmed the ITISYOU
  account (34 workers, `<name>-itisyou-app-{staging,production}` naming);
  an existing wrangler OAuth session on this machine carries
  `workers (write)` — the "existing authenticated Cloudflare access".
- Website deployed as an assets-only Worker (config committed in
  `website/wrangler.jsonc`): staging → browser-verified (home, /build with
  live verified statuses, honest 404, zero console errors, strict CSP
  headers observed) → production with custom domain **os.itisyou.app** →
  verified live: TLS, 11 routes 200, 404 handling, headers, console-clean
  browser journey. Deploy output alone was not treated as success.
- CI run 33598093709: website + gitleaks green; kernel job red at clippy —
  **Linux-only**: bitflags was built for x86_64-unknown-none with `std`
  enabled. First hypothesis (bindeps unification) was only part of it:
  removing artifact deps (subprocess kernel build — kept, it also fixed a
  `cargo tree` panic) did NOT fix CI. Actual root cause: **cargo resolves
  features once per invocation across all compile targets**, so any single
  command mixing the forced-target kernel with host packages
  (`clippy --workspace`) lets a host-only dep (present in the Linux graph,
  absent on Windows — hence the local/CI split) enable `bitflags/std` for
  the bare-metal build. Fix: `default-members` excludes the kernel from
  bare cargo commands, and every clippy/test path lints host packages and
  the kernel in **separate invocations** (ci.yml + verify.ps1).
- Full `scripts/verify.ps1` executed end-to-end afterwards: doctor, fmt,
  clippy, 41 host tests, 6/6 QEMU legs, website check+build (0 errors),
  secret scan — "VERIFY: OK".

### 07:35 — Session 1 close: everything green everywhere

- CI run 33598999034 (commit 9a1d847): **success** — the kernel boots,
  self-tests (27/0), runs its shell to B150, and panics-on-demand
  identically on the ubuntu-24.04 runner (TCG) as on the Windows host.
- Production redeployed with the stamped verified commit; live /build page
  confirmed serving commit `ceb24f9` + verification timestamp.
- V0.1 hard target: complete, verified on two firmware paths and two host
  platforms. Userspace stretch explicitly deferred to V0.2 (see
  REQUIREMENTS USR-001/ABI-001). Remaining V0.1-adjacent work: PS/2
  keyboard, timer preemption, guard pages (KNOWN_LIMITATIONS.md).

### 08:00 — V0.2 Userspace Foundation: Ring 3 is real

Baseline preserved as tag `v0.1.0`. Implemented in one dependency-ordered
pass (ADR-0004): user GDT segments + TSS RSP0; syscall/sysret MSRs with a
dedicated masked-IF kernel syscall stack; USER_ACCESSIBLE paging with
table-path flags + W^X; strict host-tested ELF64 parser; three-phase
segment loader (map+zero → copy → tighten) with window/overlap policy;
process lifecycle with full teardown; iretq entry with zeroed GPRs and a
setjmp-style abort context; CPL=3 fault containment for #PF/#GP/#UD; ulib +
three Ring 3 programs (init, gp-test, pf-test) built by a nested cargo in
kernel/build.rs and packed into the initramfs alongside deterministically
corrupted fixtures (/bin/broken, /bin/wx-test).

- Failure #1 (caught by the new selftests): all user programs failed to
  load — `x86_64-unknown-none` builds PIE (ET_DYN) by default and the
  loader only accepts static ET_EXEC. Fix: nested build uses
  `-C relocation-model=static -C link-arg=--no-pie`. Evidence discipline
  worked exactly as designed: V0.1's 29 tests stayed green while the 5 new
  ones failed with a precise cause.
- After the fix: **pass=34 fail=0**. The serial log carries the whole proof
  chain: `user_enter … ring=3` → RING3-HELLO…RING3-DONE (write syscalls
  round-tripping) → `user_exit code=0` → `#GP cs_rpl=3` on a Ring 3 `hlt`
  (hardware CPL evidence) → `#PF … USER_MODE` on a kernel-half read (MMU
  isolation evidence) → kernel alive → clean reload after teardown.

### 08:40 — V0.3 Process Isolation + Storage Foundation

Baseline preserved as tag `v0.2.0`. Implemented all five priorities in
dependency order (ADR-0005/0006/0007/0008):

1. **Per-process page tables**: `AddressSpace` — private L4, kernel entries
   1..511 shared from the boot table (supervisor-only), user window in L4
   entry 0 exclusive. Loading through the physical alias (no CR3 switch);
   recursive leak-free teardown.
2. **Concurrent processes**: unified `sysretq` enter/resume from a
   `#[repr(C)]` resumable `UserContext`; a `proc.rs` run-loop round-robins
   processes; `yield`/`wait`/`Blocked` outcomes; spawn/wait/exit.
3. **Syscall expansion**: spawn, wait, msg_send, msg_recv; `validate_user_
   range`/`copy_from_user`/`copy_to_user` against the active CR3.
4. **IPC**: bounded kernel message channels.
5. **Storage**: PCI enumeration; `BlockDevice` trait + RamDisk; a polled
   read-only **NVMe** driver.

Failures found and root-caused:
- **Triple fault at B040** on the isolation assert: the BIOS boot path
  identity-maps handoff structures in L4 entry 0, so the "user window is
  free" assumption was false. Fix: `release_boot_identity_mappings` unlinks
  entry 0 at B080 after our GDT/IDT are live (the boot GDT lived in those
  mappings, so timing matters).
- **init printed nothing under per-process CR3**: `sys_write` validated
  pointers against the *kernel* table, which no longer maps the user window.
  The selftests passed on return values but the harness caught the missing
  RING3 output — added `translate_active` (walks the live CR3). Exactly the
  kind of silent-success bug the output-assertion discipline exists to catch.
- **`&T as &mut T` in the NVMe read path** was real UB (clippy caught it):
  reworked queue cursors into an `UnsafeCell` with a documented single-CPU
  `Sync` contract.
- **NVMe init hung 150 s**: `find_storage` returned the legacy IDE
  controller (also class 0x01) instead of NVMe (subclass 0x08); init polled
  the wrong BAR forever. Fixed device selection + tightened spin bounds.

Result: **selftest pass=50 fail=0 on BIOS and UEFI**. Evidence chain adds:
per-process `l4=…` on entry; `aspace_*` isolation + leak proofs; parent
spawns children pid 10/11 that interleave (both print before either exits),
`RING3-PARENT-WAIT-OK` (both children exit 7), `RING3-PARENT-IPC-OK`;
`pci_devices count=7` incl. the NVMe controller; `nvme_ready blocks=2048`;
`nvme_disk_magic` (LBA 0 read back as `ITISYOU-OS-DISK1`). 55 host tests.

### 09:30 — V0.4 Preemptive Multitasking + Persistent Storage

Baseline preserved as tag `v0.3.0`. Two sub-milestones (ADR-0009).

**Preemption** (committed cf220dc): expanded `UserContext` to a full trap
frame and switched Ring 3 resume from sysretq to iretq (preemption interrupts
user code at any instruction, so all GPRs must be saved/restored and sysretq
clobbers rcx/r11). A naked timer ISR saves all GPRs; when a Ring 3 process's
2-tick quantum expires it saves the full trap frame and long-jumps to the
run-loop (`UserExit::Preempted`). Adversarial userspace: spin-finite (a
no-syscall compute loop verifying its own sum + stack sentinel) and
spin-forever (infinite loop). Verified pass=56/0: two non-yielding CPU-bound
processes both finish correctly (registers + address spaces preserved); an
infinite spinner cannot monopolize (a co-scheduled finite process still
completes, spinner still runnable, then reaped); cooperative yield/wait/IPC
coexist; no leaks/lost processes.

**Persistent storage**: NVMe gained write + flush; ITFS (ADR-0009) is a
minimal filesystem with double-buffered CRC-committed superblocks. Verified
pass=63/0: NVMe write→flush→read round-trip; ITFS format/create/read/list;
strict metadata validation (8 host tests + both-superblocks-corrupt reject);
crash consistency (corrupt the newest superblock → mount recovers the older
consistent slot).

- Failure (debugged at length): the dedicated `itisyou-fs-persist` binary's
  **BIOS** disk image produced ZERO serial output (not even bootloader
  lines), while structurally-identical binaries (panictest) boot fine. Its
  ELF is valid (same size/segment layout as the booting selftest) and its
  **UEFI** image boots correctly (panic captured). Root cause is a bootloader
  BIOS-stage quirk specific to that binary, not yet isolated. Resolution: run
  the reboot-persistence test on UEFI (a fully verified firmware path);
  documented in KNOWN_LIMITATIONS as a V0.5 follow-up.
- **Reboot persistence proven**: two separate QEMU guests share one
  disposable disk — boot 1 formats ITFS + writes `/hello` + flush
  (`FS-PERSIST-WROTE`), boot 2 (fresh guest) mounts + reads it back
  (`FS-PERSIST-VERIFIED`). The hard V0.4 acceptance target is met.

### 11:30 — V0.5 Graphics + Input + Basic Desktop/Compositor

Baseline preserved as tag `v0.4.0`. Goal: a real graphical environment produced
by the OS inside QEMU — desktop/compositor, keyboard + mouse, a Ring 3 GUI
window — with no faked/host-rendered UI (ADR-0010).

**Graphics foundation** (committed cd46a19): `gfx` wraps the bootloader linear
framebuffer (QEMU: 1280×720 BGR) with a `u32` back buffer + 8×8 font +
fill/glyph/blit + CRC region hash and one format-converting `present()`; the
heap grew to 32 MiB. `compositor` renders wallpaper + top bar + windows +
cursor with per-window backing stores and ownership/bounds isolation. GUI
syscalls let `user/gui-demo` (Ring 3) render a window with no direct
framebuffer access. Verified in the selftest at **pass=78/0**, including a
Ring 3 window's 0xFF8800 pixel read back off the composited screen, and
adversarial rejections (cross-owner, out-of-bounds, bad size).

- Failure (root-caused): after enabling the input IRQs the kernel hung ~300 s.
  `set_input_irqs_enabled` held the PIC lock while writing masks; a timer IRQ
  fired and the naked timer ISR spun forever trying to take the same lock for
  its EOI → deadlock. Fix: mask interrupts around the PIC write
  (`without_interrupts`), and make the mouse IRQ accumulate deltas lock-free
  instead of touching the compositor/FB lock a `composite()` may hold.

**Input + desktop**: added PS/2 keyboard (IRQ1) + mouse (IRQ12) decoders and a
`desktop` command that renders a live window and reacts to input. To prove
**real** input (not a stub), the QEMU harness gained an HMP **monitor** channel
(`--monitor`, `--inject-after`, `--monitor-cmd`): once `DESKTOP-READY` appears
it injects `sendkey h e l l o` and `mouse_move`/`mouse_button` into the emulated
PS/2 devices. The kernel's IRQ handlers observed them
(`[ITISYOU:INPUT] key=h..o`, `mouse dx=-20 dy=-15`, `l=1`→`l=0`), the desktop
updated live, and `DESKTOP-INPUT-VERIFIED keys=5 mouse=4` gated success. A
framebuffer `screendump` captured the OS-rendered desktop (its pixels match the
compositor palette exactly) — the UI is produced by ITISYOU OS inside QEMU, not
the host.

- Environment note: BIOS boot in this session's QEMU/TCG is slow (~40 s to load
  the bootloader stages via ATA PIO; UEFI ~12 s). Early diagnostics with short
  read windows looked like a "boot regression"; a longer wait showed the
  bootloader's 4th stage + framebuffer info arriving normally and the selftest
  passing 78/0. Timeouts were sized accordingly (no acceleration change, to
  preserve the exact validated V0.1–V0.4 behavior).
- The first PS/2 mouse packet after enabling reporting can carry a benign
  zero-motion sync artifact; the decoder resynchronises and every later packet
  decodes exactly (documented in KNOWN_LIMITATIONS).
- **Closed:** CI run 33628363046 green on ubuntu-24.04 (the desktop-input
  monitor-injection leg passes identically under Linux QEMU); tagged `v0.5.0`
  on commit `5be3c0c`; os.itisyou.app redeployed and browser+HTTP verified
  (v0.5.0-dev, OS-rendered desktop screendump on /build, zero console errors).

### 12:40 — V0.6 Hardware Expansion

Baseline preserved as tag `v0.5.0`. Goal: turn the QEMU desktop OS into a real
hardware platform — a device/driver model, PCI depth, USB, audio, input
unification (ADR-0011).

**Device model + PCI** (committed a180298): every PCI function is probed into a
`Device` (identity + sized BARs + capability list). BAR sizing writes all-ones
and restores (non-destructive, verified). The capability walker is bounded and
loop-guarded; host tests cover circular/self-loop/out-of-range chains. A
`Driver` trait + explicit registry binds drivers deterministically at a new
B190. QEMU showed 7 real devices; the NVMe controller's caps decoded as
`[MSI-X,PCIe,PM]`. `lsdev` shell command + selftest pass=83/0.

**Audio** (committed 2ae29e5): an AC97 driver — codec reset/unmute, a bus-master
BDL over DMA frames of a synthesized 440 Hz tone. The insight that made audio
*honest*: QEMU's `wav` audio backend writes played samples to a file, so the
harness asserts the captured WAV is **non-silent**. `ac97 play bufs=8 civ=7
halted=true` (all buffers DMA-consumed) + a WAV full of `0x1fff` tone samples =
real end-to-end audio, not init-only.

**USB** (committed e089fb3): a UHCI driver. The biggest lift — frame list, QH,
TDs, control transfers. Chose UHCI over xHCI precisely because it's the smallest
controller QEMU emulates fully with `usb-kbd`, so HID input could be *verified*.
It enumerated the real QEMU keyboard (`usb device vendor=0x0627 product=0x0001`),
parsed its config descriptor to find the HID interrupt endpoint, set address +
configuration + boot protocol, then read a report off the interrupt endpoint:
injected `sendkey a` → `[ITISYOU:INPUT] usb key=a`. Real USB HID input.

**Input unification + userspace** (committed 1a5e7d8): USB HID and PS/2 now
decode into one `InputEvent` queue; the desktop pumps USB each loop and reacts
to `key=g src=usb` identically to PS/2. A `devinfo` syscall gives Ring 3 the
device table without any hardware authority; a userspace `/bin/lsdev` proved it
(`RING3-LSDEV-OK count=7`). Selftest pass=84/0.

- Decision: every V0.6 driver is **polled**. MSI/MSI-X capabilities are detected
  and reported, but wiring APIC/MSI would mean rewriting the verified PIC
  timer/PS-2 interrupt path's tests — a regression risk, not progress. IRQ
  modernization is deferred to V0.8 with that rationale recorded (ADR-0011).

### 17:00 — V0.7 System Platform

Baseline preserved as tag `v0.6.0`. Goal: turn the foundation into a coherent
platform — capabilities, services, apps/packages, updates, recovery, audit
(ADR-0012) — with the AI-authority rule built into the bones.

**Capability model** (committed cda6343): explicit per-process bits enforced
default-deny at the syscall dispatch boundary; `spawn` inherits exactly,
`spawn_caps` intersects — amplification is impossible by construction and
proven adversarially (a gui-less parent requesting gui for its child yields a
child whose gui calls are ERR_PERM while its delegated fs_read works). A new
`fs_read` syscall carries the FS sandbox: normalized-path prefix checks that
defeat `..` traversal, `//`, and component-boundary tricks. The
sandbox-probe program attempts all 7 privileged syscall classes with zero
capabilities and exits 0 only if every one was denied — SANDBOX-DENIED-OK is
the machine-readable proof of default deny. Every denial is audited.

**Services**: a static registry of Ring 3 services with declared deps + exact
capabilities; the supervisor computes a deterministic cycle-checked startup
order (host-tested Kahn), co-schedules preemptively, contains crashes, and
applies the bounded restart policy. Verified: echod actually SERVED a
dependent client (3 IPC ping→pong), crashd faulted, was restarted exactly 3x,
then marked Failed — no restart storm, no leaked processes.

**Packages/updates**: ITPKG (strict manifest + ELF + SHA-256, packed by
build.rs with the same kernel-core code the kernel verifies with). The
persistent store's every transition is ONE crash-atomic ITFS superblock
commit (ITFS gained atomic `remove`): install = stage + commit-marker;
rollback = remove the newest marker (previous version reactivates, demoted
package kept as evidence); recovery detects + audits + removes orphaned
staged updates. Corrupted (bit-flipped) and hostile-manifest (undefined
capability) packages are refused with the store untouched. hello-app
launches with ONLY its manifest capability — its out-of-manifest gui attempt
is denied even though the binary asks.

- Failure (root-caused fast thanks to a missing marker): the second install
  in the selftest silently failed — the 774 KB debug ELF nearly filled the
  1 MiB disposable NVMe disk, so staging v2 hit NoSpace. Fixed by growing
  the harness test disks to 16 MiB and making storage errors emit a marker
  (silent failure paths are themselves bugs).
- **Reboot-proven recovery**: boot 1 installs v1 and stages v2 without
  committing (simulated crash mid-update), powers off; boot 2 finds v1
  active, detects the orphan, removes it via the recovery scan, launches v1.
  An interrupted update can never activate — across a REAL reboot.

Selftest pass=106 fail=0 (all V0.1–V0.6 suites green under the new
enforcement); kernel-core at 122 host tests; new platform-bios +
update-interrupt/update-recovery QEMU legs.

### 2026-09-03 · V0.7 evidence closeout

The final local `scripts/verify.ps1` gate remained green: formatting, split
clippy, 41 host tests, all six QEMU legs, website check/build, and secret scan.
The corresponding GitHub Actions CI run `33810268090` completed successfully
on ubuntu-24.04 for commit `1f08bf5`. The QEMU artifacts include the platform
security leg and the persistent-disk interrupted-update/recovery pair.

The website was verified in the required order: staging first at
`os-itisyou-app-staging.kpleelaaravind.workers.dev`, then production at
`https://os.itisyou.app`. The production check covered V0.7 platform content,
status metadata, TLS/security headers, route/404 behavior, responsive CSS,
browser navigation, and zero console errors. `status/current.json` is now
stamped to the verified V0.7 state; the release tag follows the final pushed
evidence commit. The final deployment versions were staging
`a816d6cd-0d6b-48a8-a87c-ac3c1fa1c353` and production
`bf677d19-6f34-4492-a37a-5c33e5a7200d`.

### 2026-09-04 - V0.8 capability-handle foundation

V0.7 was preserved as the immutable `v0.7.0` baseline. V0.8 work began with
an allocation-free `CapabilityTable` in kernel-core: opaque generation
checked handles, owner/resource scope and rights checks, bounded delegation,
revocation, expiry, and automatic owner teardown revocation are covered by
four direct host tests. Runtime kernel/QEMU integration remains deliberately
unstamped until it is exercised at the actual boundary.

### 2026-09-05 - V0.8 long-running services, and pacing that means something

The V0.7 supervisor could only run services to completion, which quietly made
one thing impossible: a client could never talk to a service, because the
service was not running while the client was. `bg` fixed that by pumping the
scheduler until the job ends instead of running it alone, and the shell's idle
loop — which had been spinning on `spin_loop()` waiting for a serial byte —
now spends that time giving the daemons CPU. The shell is waiting on a human
either way.

Two things then had to be defined rather than assumed. First, what failure
means for a process that is not supposed to finish: `flapd` exits cleanly and
immediately, and the supervisor must treat that exactly like a crash, restart
it under the bounded policy, and stop at the ceiling rather than forever.
Second, how a daemon paces periodic work. `tickd` originally counted its own
scheduling passes, and the first QEMU run showed why that is meaningless: 243
of the 397 serial lines were heartbeats, because pass rate measures system
load, not elapsed time — the same daemon emitted two heartbeats while the
machine was busy and hundreds per second while it was idle. `SYS_UPTIME` (a
free-running tick counter, ungated because it conveys no authority) replaced
the pass count, and the cadence became a flat 2 s: 12 heartbeat lines instead
of 243.

Two smaller defects surfaced from actually reading the output rather than the
code. `run_supervised` cleared the whole status table, erasing the live
background rows on every `svc`; and it counted every row in its report, so a
failed background daemon showed up as `started=3 failed=2` — arithmetic
nonsense. Both now scope themselves to the on-demand registry.

The untracked network parsers were wired into `kernel-core` as
`net::{checksum,eth,ipv4}` — 43 host tests for VLAN tags, fragments, bad
IHL/TTL/checksum and every truncation boundary — and clippy/fmt cleaned. They
move no packets, and the docs say so: there is no NIC driver yet.

Local evidence: selftest pass=113 fail=0, 181 host tests, 15/15 QEMU legs
Success including the new `services-bg-bios` leg.
