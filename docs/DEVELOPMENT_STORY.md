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


### 2026-09-05 - V0.8: the first milestone whose inputs come from elsewhere

Everything up to V0.7 processed bytes this machine produced: an image the build
made, a disk the harness generated, a keypress the harness injected. A network
stack is the first subsystem that parses input chosen by someone else, arriving
before any authentication exists to judge it by. That framing shaped the whole
milestone.

**The network is verified against something other than itself.** The harness
brings its own Ethernet peer over a QEMU `dgram` netdev and *is* the entire
network the guest sees - no slirp, no host resolver, nothing outside this
machine. It is deliberately an independent byte-level implementation rather
than a second use of `kernel_core::net`, because a test where both ends share a
checksum routine proves the two agree, not that either is right. It also probes
the guest with an ARP request and a ping: a stack that only ever initiates is
not a host on a network. Then it sends five frames a correct stack must refuse
- a corrupt IP checksum, a ping addressed elsewhere but delivered to our MAC,
UDP to an unbound port, an 802.1Q tag, and an ARP whose hardware type
contradicts its address lengths - and the assertion is that the guest counts
them as refused AND answers none of them.

Three bugs came out of running it rather than reading it:

- `match IFACE.lock().arp_lookup(hop)` keeps the guard alive for the whole
  match, and the `None` arm re-locks to send the request. A self-deadlock on a
  spin lock, with interrupts off: a dead machine, from a line that reads
  perfectly.
- Bounded waits were built on the timer tick, which does not advance inside a
  syscall because `SFMASK` clears IF on entry. A one-second timeout therefore
  burned its entire 400-million-spin backstop. The fix was a TSC calibrated
  against the PIT - the only clock that advances with interrupts off - plus
  making the datagram syscalls non-blocking so waiting happens on the caller's
  own scheduling slice, where yielding is free.
- A tight userspace retry loop emitted one ARP broadcast per attempt: 879 in a
  single run.

**Ed25519, written out, with the constants derived.** Package authenticity
needed a signature scheme, and it lives in `kernel-core` alongside SHA-256 for
the same reasons: no dependency, no allocator, host-testable. It is validated
against RFC 8032's own vectors rather than only against itself, and every curve
constant is computed from small integers at use time. A mistyped 32-byte
constant yields a working implementation of a *different curve*: self-
consistent, passing every round-trip test, and unable to verify a single real
signature. Integrity and authenticity are separate steps so that a corrupt
download, a package from a stranger and a forged signature stay three
distinguishable refusals.

**The hardening had to be allowed before it could be proved.** QEMU's default
`qemu64` model advertises neither SMEP nor SMAP nor UMIP, so enabling them in
the kernel would have been enabling them into a void. The runner now requests
`+smep,+smap,+umip`, which means every leg runs with supervisor-mode protection
on - and the other 23 legs passing is itself the evidence that the kernel's own
legitimate access to user memory still works. SMAP inverts the default: instead
of the kernel being allowed to touch user memory everywhere and being careful
not to, it is forbidden everywhere and has to say where it means to. There
turned out to be exactly three such places.

The harness also gained `--forbid`. Two of the hardening assertions are about
*absence*: "the CPU refused" prints no line of its own, so the only way to
state it is that the marker printed on success never appeared.

**What was deliberately not built.** TCP. A correct TCP needs a retransmission
timer, window management and a connection state machine, and those are exactly
the parts a half-implementation hides. It is recorded as NOT DONE in the
requirement matrix rather than reworded into something that sounds finished.
The I/O APIC is programmed and read back but its entries stay masked, so line
IRQs still run on the verified PIC path - and the evidence line says
`masked=true` precisely so it cannot be misread. The package trust root is a
development key whose seed is published in the source tree; that is stated
everywhere it matters, because a build-time key that looked secret would invite
someone to trust it.

Local evidence: 24/24 QEMU legs Success, selftest pass=113 fail=0, 278 host
tests.

---

## 2026-09-13 — Session 4: independent audit, then V0.8.1 (release-integrity closeout)

**Starting state.** `main` = `v0.8.0` = `84d6b24`, clean, CI green. The session
brief was to establish the true state from evidence — runtime first, then code,
then Git/CI, then documents — before building anything, then take the project
version by version toward V1.0. Mid-session the owner set a strict six-hour
deadline (hard stop 15:44 BST), which bounds how far this session goes.

**The first failure was the environment, not the code.** `G:` (the external
scratch drive) was not attached, and `scripts/env.ps1` created
`G:\claude-tmp\tmp` unconditionally, so every gate script died before doing
anything. The fix uses `G:` when present and `E:\claude-tmp` otherwise, and the
doctor now checks that `TEMP` is off `C:` instead of demanding a drive that
may be absent.

**Runtime evidence before documents.** With that fixed, the full gate ran on the
tagged commit: `VERIFY: OK`, 24/24 QEMU legs, selftest pass=113 fail=0 on BIOS
and UEFI, 292 host tests (278 in kernel-core), website clean, secret scan
clean. The V0.8 engineering reproduces on today's toolchain.

**The release did not agree with itself.**

- The `v0.8.0` kernel printed `itisyou-os 0.7.0-dev`. The workspace version had
  not been bumped since V0.7 — and both shell-test legs `--require`d the same
  stale string, so the one test that could have caught the drift pinned it in
  place. A test that asserts whatever the code currently prints is a snapshot,
  not a check.
- NET08-003 (TCP) and NET08-004 (DHCP/IPv6) were "NOT DONE", a state the plan
  does not have. They were deferred to V0.9 by an explicit roadmap entry, so
  the honest terminal state is `NOT APPLICABLE` naming that amendment, with the
  work carried forward as NET09-001..003.
- The live site said "no network stack exists in V0.1" on the security page,
  presented the verified capability model as a "future concept", and described
  release rules for V0.1. The README said the current milestone was V0.1.
  KNOWN_LIMITATIONS contained V0.7-era bullets ("capabilities are bits, not
  handles", "no filesystem write syscall", "SMEP/SMAP not enabled") directly
  above the V0.8 bullets that contradicted them. ADR-0013 said integration was
  pending. The V0.8 APIC, e1000, xHCI and SMAP `unsafe` code had no inventory
  rows.
- The V0.8 site had been verified over HTTP only; the browser extension was
  unavailable, which was recorded, but no browser ever looked at it.

**Why a patch release.** A tag is immutable and a release whose artifact
misnames itself is a defect, so the correction is `v0.8.1` rather than a moved
tag. It changes no kernel behaviour beyond the version string.

**What makes it not recur.** `website/scripts/check-consistency.mjs` runs in
every website build — so in `verify.ps1` and every CI run — and fails when the
status version differs from the Cargo workspace version the kernel reports,
when a requirement state is outside the vocabulary, or when a `verified` status
carries unstarted rows; `--release` refuses any non-terminal row before a tag.
Run against the `v0.8.0` tree it exits 1 naming exactly the three defects that
shipped, which is its regression test. `website/scripts/browser-verify.mjs`
drives the installed Chrome headless over the DevTools protocol — no new
dependency — and fails on console errors, CSP or blocked-resource log errors,
failed requests, horizontal overflow at 1440 and 390 px, broken internal links,
a wrong 404, or missing security headers. `crates/kernel-core` is now
`#![forbid(unsafe_code)]`, which it already satisfied by convention.

**Roadmap amendment.** The sequence after V0.8 was re-derived from
dependencies rather than kept as written: the AI layer assumes an agent that is
an ordinary supervised Ring 3 service, but today processes only run while the
kernel console drives the scheduler and there is no userspace init — so a
Userspace System milestone (V0.10) now precedes the AI layer (V0.11), and
daily-driver hardware research moves after V1.0 because it cannot be verified
in QEMU and needs a per-device hardware authorization. Recorded in
`docs/ROADMAP.md` with the reasons.

**Left alone deliberately.** Draft PR #1 (`W0-01: Freeze normative inputs`)
proposes a different architecture chain whose source documents are not in the
repository; it is unmerged and owner-gated, so this session did not touch it.

### The first public download, and what publishing it exposed

The owner's brief asks for a real downloadable image, so V0.8.1 publishes one —
and each step of doing it properly found something:

- **Two builds, two different UEFI images.** The BIOS image rebuilt
  bit-identically; the UEFI image did not. The partitioning crate the
  bootloader uses stamps a *random* disk GUID and partition GUID into every
  GPT. A checksum that changes on every rebuild cannot be independently
  checked, so `tools/image-builder/src/gpt_normalize.rs` now derives both GUIDs
  from the image's own content and recomputes every GPT CRC (five host tests,
  including one proving that two images with different random GUIDs normalize
  to the same bytes). All eight images now rebuild identically.
- **A Windows build is not a Linux build.** Rust embeds panic-location paths,
  and on this host those include the local rustup `rust-src` path and `\`
  separators. Cross-OS byte identity would need path remapping the toolchain
  does not fully offer, so the published bytes are the CI-built ones: the
  workflow prints their digests before any QEMU leg runs and uploads them as the
  `boot-images` artifact, and the release downloads exactly those, compares
  the hashes, and boots them. A new CI job builds the images twice from two
  clean checkouts in different directories and fails if any digest differs.
- **Booting an image changed it.** After the first release boot test the UEFI
  file no longer matched its SHA-256. OVMF with no writable variable store
  writes an `NvVars` file into the EFI partition on every boot — so every UEFI
  test leg had been quietly modifying the image it was testing. The runner now
  attaches the boot image with `snapshot=on`, the release boot test checks the
  files are unchanged afterwards, and the download page tells users to verify
  *before* booting and uses `snapshot=on` in its commands.
- **The network instructions were tested before being written down.** On QEMU's
  user-mode network the image resolves the gateway and gets echo replies. DNS
  through QEMU's forwarder did not work on this host (no UDP reply at all), so
  the page says UDP and DNS are verified only against the harness's own peer —
  and the failure is carried into the V0.9 network work rather than hidden.
- **The stamp commit failed CI.** `8af917c` changed the runner after the local
  format check had run, and the new line was over rustfmt's width. CI refused it
  at the first step. The fix is the formatting; the lesson is that a partial
  re-check after a late edit is not the gate.

The site was then deployed to staging, verified by the new headless-Chromium
verifier (28 page loads across both widths, 24 internal links, zero console
errors or failed requests) and by re-downloading both images and checking their
bytes and headers, and only then deployed to production and verified again the
same way.

**Reproducible, second attempt.** The GPT fix made two builds in one tree
identical, which turned out to prove less than it seemed: both builds reused the
same cached UEFI loader binary. Two clean builds from two fresh clones still
disagreed — in exactly ten bytes, all inside the embedded `BOOTX64.EFI`: the
link time in its COFF header and debug directory, and a CodeView GUID derived
from it. `pe_normalize.rs` now zeroes those fields and derives the GUID from
the file's content; the two real, independently built images normalize to the
same bytes and still boot under OVMF. The UEFI image already uploaded as
release candidate 1 could not be reproduced from source, so it is superseded
before the tag rather than left as the release — and the download page says so
instead of swapping the file silently. A new CI job builds every image from two
clean checkouts in different directories and fails on any difference, so the
property is now checked on every push rather than asserted once.

---

## 2026-09-13 — Session 4, continued: V0.9 begins (interrupt cutover, power-off, DHCP)

**Starting state.** `v0.8.1` tagged on `6c5415c`; working version `0.9.0-dev`.

**ACPI first, because the cutover depends on it.** V0.8 could only program a
masked I/O APIC entry at the architectural address; it did not know that QEMU
delivers the PIT's IRQ0 on GSI 2. `kernel_core::acpi` parses the RSDP, the
RSDT/XSDT, the MADT and the FADT, and finds `\_S5_` in the DSDT by pattern —
refusing zero-length MADT entries (an infinite loop for a naive walker), tables
whose lengths or checksums lie, and any `\_S5_` it would have to guess at. The
BIOS and UEFI firmware present genuinely different tables (ACPI 1.0/RSDT/PM1a
`0x604` versus 2.0/XSDT/`0xb004`), so both paths are exercised.

**The cutover worked the first time — and was wrong.** Timer, keyboard and mouse
moved to the I/O APIC with their old vectors, both PICs and LINT0 masked, every
leg green. But the new `irq` delivery proof counted 41 ticks in 200 ms where
100 Hz gives 20, so the cutover now measures the tick rate on both sides of
itself: `before=10 after=20`. QEMU's edge-triggered I/O APIC was delivering the
PIT's mode-3 square wave twice per period. Nothing functional failed — every
quantum and every deadline would simply have been half as long. The PIT now
runs in mode 2 (rate generator, one edge per period; what Linux uses):
`before=10 after=10 rate_preserved=true`. The lesson: an interrupt path is not
verified by "the machine still boots"; it is verified by counting.

**The adversarial half of the proof.** With the timer's I/O APIC entry masked
the tick count must stop dead (`masked_ticks=0`) and resume when unmasked — the
only way to show nothing else is still delivering the timer.

**Power-off is real now.** `poweroff` writes `SLP_TYP|SLP_EN` to the FADT's PM1
control block, enabling ACPI mode through `SMI_CMD` first where firmware has not
(SeaBIOS hands over in legacy mode, OVMF in ACPI mode — both paths seen).

**DHCP exposed a V0.8 bug.** Against QEMU's own DHCP server the client sent
DISCOVERs and received five ~590-byte replies, all counted `rx_malformed`. The
IPv4 layer was handing the UDP layer the host's *own* address as the checksum
pseudo-header's destination instead of the destination in the header, so every
broadcast datagram failed its checksum. V0.8's harness peer only ever sent
unicast, so the bug was invisible. Fixed; the DHCP leg is the regression test.
That leg runs QEMU's DHCP server on 10.0.9.0/24 — not the guest's static plan —
so the gateway is unreachable before the lease and reachable after, and the
address can only have come from DHCP.

**Anchoring the audit head — shown by attacking it first.** The roadmap item
was "sign the audit head"; the amendment changed it to "anchor it", because a
key stored beside the log protects nothing from an attacker who can rewrite the
disk. The test demonstrates the weakness before the defence: boot 3 overwrites
the trail with a perfectly valid *empty* one (`count=0`, genesis head), and
boot 4's recovery reports `status=verified records=0` — the chain cannot tell,
because there is nothing in it to be inconsistent with. The fix is a copy of
the head somewhere else: `audit anchor` sends the saved head to a witness that
must echo exactly what it stored, and `audit check-anchor` compares the
witness's copy with the trail recovered at boot — `MISMATCH`, recorded as a
denial. In the harness the witness is the runner itself, persisting anchors to
a file across the four separate QEMU processes; on a real deployment it would
be another machine, which is the whole point.

**IPv6 foundations, and the half QEMU could not test.** The IPv6 codec
(`kernel_core::net::ipv6`: header, ICMPv6 checksum over the pseudo-header,
echo, RS/RA/NS/NA with RFC 4861 validation, RFC 5952 formatting) was written in
parallel by a subagent in an isolated file and reviewed before integration. The
kernel side derives its link-local address from the MAC and joins exactly two
multicast groups in the e1000's hash filter — all-nodes and its solicited-node
group — rather than switching the card to multicast-promiscuous mode, keeping
the V0.8 rule that the card filters and the stack re-checks. Against QEMU's own
IPv6 router it formed `fec0::5054:ff:fe12:3456` by SLAAC and pinged the router,
first run. But QEMU never solicited the guest, so the guest's *responder* paths
(answering neighbour solicitations and echo requests) had run zero times —
`neighbor_adverts_sent=0` said so. The harness's own byte-level peer now
solicits the guest and pings it, checking every ICMPv6 checksum with its own
code. One design correction during integration: the echo-reply path originally
resolved the destination with a blocking neighbour solicitation from *inside*
the receive path, which would have re-entered `poll` from itself; it now uses
the cache that the solicitation which preceded the ping has just filled.

**TCP, tested against a stack we did not write.** The state machine
(`kernel_core::net::tcp`) was written by a subagent in one isolated file against
a specification — RFC 9293 transitions, RFC 5961 reset and SYN handling, one
retransmission timer with backoff — and it found one bug of its own on the way
(a FIN queued behind unsent data was lost if the peer's FIN arrived first). The
kernel side keeps eight connection slots and drives every timer from the NIC
poll. The test peer is the point: QEMU's user-mode network maps the guest's
10.0.2.2 to the host's loopback, so the guest talks to the *host operating
system's* TCP stack, and the harness's echo endpoint checks the byte pattern
with its own code. A test-only switch (`tcp drop 2`) discards the next two
outgoing data segments, so retransmission is exercised against that real peer
instead of being claimed from unit tests.

The first QEMU run failed in a way worth recording. The connect succeeded; the
very next syscall was refused with `invalid_handle` — a capability handle that
had been valid a moment earlier. Nothing in TCP touches capabilities. The
cause: a TCP connection block is 8 KB, and the unoptimized build copied it by
value several times on its way into the table — more than the 32 KB syscall
stack holds. The stack has no guard page (a known limitation), so the overflow
did not fault; it wrote over whatever sat below the stack, which happened to be
the capability table. The fix builds connections in place in their static
slots (`Tcb::connect_in_place`, host-tested to leave no trace of the slot's
previous connection). The lesson stands in `docs/KNOWN_LIMITATIONS.md`: without
guard pages, a kernel stack overflow is silent corruption, and this one
surfaced only because the corrupted bytes happened to be checked.

The second finding was older. After a probe exited in TIME-WAIT, its connection
still named the dead process as owner. The console's foreground `run` path had
never released a program's network resources — only the scheduler's exit path
did — so since V0.8 a foreground program that exited without closing its UDP
socket left the port bound for the rest of the boot. Both paths now release
sockets and connections, and the leg shows it (`owner_exit pid=3 orphaned=1`).

**A key hierarchy, and the question that was blocking it.** The recorded design
question for KEY09-001 was real: the image's fixture packages are signed at
build time, so moving the signing key out of the tree seemed to require a CI
secret — and an image nobody else could rebuild bit for bit, undoing v0.8.1's
reproducibility. The answer was to stop treating "the key that signs the
fixtures" and "the key the kernel trusts" as the same key. The kernel now
trusts only an offline root (its public half compiled in; its private half
generated with the OS CSPRNG and kept outside the repository). The fixture key
stays published, but it is trusted only through a root-signed certificate that
covers package names starting `hello-` — so it can sign fixtures and nothing
else. A release key, also off-tree, is certified for any name. Validity windows
are in kernel release epochs rather than dates, because this kernel has no
clock an attacker cannot set. The QEMU leg exercises every branch with intact,
correctly signed packages that only the chain can refuse: a revoked key, an
expired certificate, a certificate signed by an impostor root (refused when
`/etc/trust` is loaded), and the published test key signing a package outside
its scope. The last is the one that makes publishing that key defensible.

**The CI that would not start.** The IPv6/TCP checkpoint was pushed and every
CI job was refused before it began: GitHub reported that the account's
payments had failed or its spending limit needed raising. That is outside
anything this session can or should touch. The verification of record for the
V0.9 checkpoints is therefore the full local gate, run in an isolated worktree
at each exact commit; the V0.9 release waits for CI to run again.

**Closing the hole the TCP bug fell through.** The stack overflow that
corrupted the capability table was fixed where it happened, but the class of
bug was still open: any future path that ran the syscall stack dry would do
the same thing, silently. Both static kernel stacks now sit on a page-aligned
guard page that is unmapped once the double-fault stack is live. The subtle
part is where the fault lands: a page fault on the guard page cannot be
delivered, because delivering it means pushing onto the stack that just ran
out, so the CPU escalates to a double fault, which has its own stack; the
handler reads CR2, sees the guard page, and names the stack that overflowed.
The test does it on purpose — the console switches onto the syscall stack and
recurses — and the leg requires the machine to stop with
`kernel_stack_overflow stack=priv`. Heap-allocated task stacks are still
unguarded, and the limitation says so.

## 2026-09-13 — V0.10: program arguments (PROC10-001)

Until V0.10 a Ring 3 program could not be told anything at launch: every
variation was a separate binary in `/bin`. Each process now carries one
immutable argument block — at most 16 arguments and 512 bytes, each argument
printable ASCII without spaces, each followed by a NUL — validated once by
`kernel_core::progargs` when the process is built and never changed after.
The narrow alphabet is deliberate: an argument is exactly one console token, so
there are no quoting rules to get wrong, and no control byte a program could be
handed to replay onto the console. A program reads its block with `args`
(syscall 35), which needs no capability because it returns only what the
launcher chose to give it; the copy is all-or-nothing — a buffer one byte
short gets `ERR_2BIG` and is left untouched, since a truncated block could end
mid-argument and read as a shorter, different list. The user-entry path was not
touched: arguments travel through a syscall, not the initial stack.

Two launch paths, one validator. The console takes `run <path> [caps|-]
[prefix|-] -- <args>` (and the same after `bg`); a parent uses `spawn_args`
(syscall 36), which is `spawn_caps` plus a block and keeps its capability gate
and delegation rule exactly. Two things had to move for the console path to be
testable at all: the tokenizer's bound was eight tokens, which would have
refused 17 arguments before the argument validator ever saw them, so it is now
24 and a compile-time assertion keeps it above the argument limit; and the
parent→child proof has to use `bg`, because `run` executes one program alone
and a `wait` there returns 0 before the child has run — so the child exits 42
only after checking its arguments, and the parent requires 42 rather than
trusting any zero. The QEMU leg also hands `spawn_args` six hostile blocks
(too many, too long, a control byte, a space, an empty argument, no
terminator); each is refused with its own error before anything is loaded, and
the leg forbids any sign that a child was created.

## 2026-09-13 — V0.10: ITFS stops leaking space (FS10-001)

ITFS had leaked every removed or overwritten extent since V0.4, on purpose:
a bump allocator never has to prove that the block it hands out is unused.
Reuse turned out not to need a free list at all. Every file is one
contiguous extent and the superblock lists every live one, so free space is
just the gaps between them, recomputed from the directory on demand — the
on-disk format did not change. The work was in saying precisely which gaps
are safe. The obvious answer, "whatever the committed superblock does not
reference", is not enough: an overwrite built as remove-then-allocate on the
working copy would be handed the file's own old extent, still the only good
copy until the commit lands. So an overwrite chooses its new run while the
old entry is still in the directory. And the double buffer means a mount can
fall back to the OTHER slot, so its extents are pinned too: an extent is
reused only once neither slot references it, which keeps the V0.9 property
that whichever valid superblock a mount picks points at intact data. The
kernel re-checks that against both slots before writing a single data block,
independently of the allocator. The QEMU leg writes 21 one-block files' worth
into a disk with 14 data blocks — the old allocator would have failed on the
15th — and both boots print a space report whose numbers a host test
predicted before the run. What remains is stated: files are still
contiguous, so a write larger than every gap is refused as `Fragmented` even
when enough blocks are free in total; there is no compaction.

**Wind-up, under a hard deadline.** The owner set a strict six-hour box for
this session (09:44–15:44 BST). By the wind-up every V0.9 feature row was
verified by the full local gate at an exact commit, the public site had been
corrected (it still called V0.9 "planned") and re-verified in a real browser
with the v0.8.1 download re-checked byte for byte, and two V0.10 items —
program arguments and ITFS space reclamation — had been built by parallel
agents in isolated worktrees, each with its own QEMU evidence and a negative
control, then merged onto a separate integration branch and gated again
there. They stay off `main` deliberately: `main` is the V0.9 release
candidate, and the only thing between it and `v0.9.0` is a CI run GitHub will
not start until the account's billing is fixed. That is written down as the
first next action in `docs/SESSION_CHECKPOINT.md`, with the rest of the
release runbook after it.

**Guarding the stacks V0.10 will lean on.** HARD09-001 guarded the two static
kernel stacks and said plainly that task stacks — heap `Vec`s — were not. The
always-on scheduler planned for V0.10 will run real kernel tasks on exactly
those stacks, so they moved first: each task gets a fixed slot in a dedicated
virtual window, the slot's first page never mapped. The leg spawns a task that
recurses and requires the double fault to name it.

**v0.9.0, and the repository that went public for an hour.** With CI refused
for billing, the owner chose the free path: make the repository public so
GitHub-hosted runners would run, release, then make it private again. Before
flipping it the whole history was scanned — all 77 commits on every branch —
for credential patterns and for sensitive file names; nothing but the secret
scanner's own filename matched, and the signing roots live outside the
repository by design. The release commit's CI run went green on the first
try: the whole QEMU matrix on Linux, including every V0.9 leg that had only
ever run on Windows, and the reproducibility job, whose two clean builds landed
on exactly the digests the build job uploaded. The published images were then
boot-tested byte for byte (UEFI and BIOS reporting `itisyou-os 0.9.0`,
two-boot persistence, networking) before the site linked them, and v0.8.1 was
kept downloadable beside them.

**Two V0.9 defects, each shown before it was fixed.** The V0.10 design review
read the entry paths closely and found that a Ring 3 program's direction and
alignment-check flags survive interrupt delivery: the naked timer ISR and the
fault-return path never cleared them, so after a preemption the kernel ran
with string instructions reversed and SMAP switched off. A probe that sets
both flags and spins made the unfixed kernel record 75 dirty timer entries
and 38 dirty returns to the run-loop; with the flags cleared on every entry
from Ring 3 the counts are zero across 108 checks. The same review showed the
V0.9 stack-guard claim was wider than the code: syscalls have a stack of their
own, and it is now on a guard page too, with a test that overflows that stack
rather than a neighbouring one.

**A second, shorter window.** The owner reopened the public window for an hour
to get CI working again, and moved the signing keys onto an external drive to
keep offline. The flaky leg turned out to be a test asserting a timing
accident rather than a property: whether the probe's connection was still in
its one-second TIME-WAIT when the program exited. On a fast laptop it always
was; on a busy CI runner it sometimes was not, and the kernel correctly freed
the slot earlier. The leg now checks the thing that matters — nothing is left
owned by a program after it exits — and `main` went green. The V0.10 branch,
brought up to date with the release, ran on GitHub's Linux runners for the
first time through a draft pull request and passed everything.

**The last hour.** With CI working again only while the repository was
public, the owner's one-hour box was spent on what could be finished and
verified inside it: the release reviewers' leftover findings about rendered
pages that still described an older system — a security model claiming no
network stack, an interrupt section from before the PIC was retired. Those
were fixed, deployed, checked in a browser, and given a green CI run in a
third six-minute public window before the repository went private again.
V0.10's next items — a userspace init with an always-on scheduler, a Ring 3
shell, desktop applications — are larger than an hour and are left for the
next session rather than started and abandoned half-built.

**A correction to the stack-guard story.** Reading the code for V0.10's
scheduler design, a reviewer found that the V0.9 guard-page work guarded the
wrong stack for the bug that motivated it. Syscalls do not run on the RSP0
stack; they run on a third static stack of their own, which is where the TCP
connection copies overflowed — and that stack still has no guard page. The
V0.9 entry above says "the console switches onto the syscall stack"; it
switches onto RSP0, which is guarded and verified, but it is not where
syscalls run. Every public statement of the wider claim was corrected on
the website and in the requirements the same afternoon, and the missing guard
is the first item of V0.10. The same review found a second latent flaw: the
naked timer interrupt path does not clear the direction and alignment-check
flags a Ring 3 program may have set before kernel code runs after a
preemption; that is also scheduled for V0.10, with a negative control.

**The serial log is the evidence, so it has to be trustworthy.** The design
review noticed two ways a Ring 3 program could corrupt the log that every test
reads: output written in pieces could be interleaved with anyone else's, and
nothing stopped a program from printing a line that looks exactly like a
kernel marker. A probe showed both on the old write path — two lines written
a byte at a time came out as `NEEPPRROOBBEE-` and `00112233…`, a kernel
`user_exit` marker landed in the middle of a user's line, and a forged
`[ITISYOU:SVC]` line went straight through. Each process now has a small line
buffer and each complete line reaches the port in one locked write; any
`[ITISYOU:` a program prints becomes `[RING3-U:`. This had to come before the
always-on scheduler, which will make the interleaving routine instead of rare.

**A denial of service in every release so far.** Writing V0.10's
`wait_nohang`, which returns a status into a buffer the program names, the
question "what if that buffer is read-only?" led back to the kernel's one
user-copy helper. It checked that each page of the buffer was mapped, never
that the program could write it. A program that points `cap_list` — which
needs no capability — at its own code makes the kernel write there in Ring 0;
the write-protect bit turns that into a page fault, and a kernel-mode page
fault panics. A small probe showed it on the V0.10 kernel before the
fix: `[ITISYOU:PANIC] page fault … PROTECTION_VIOLATION | CAUSED_BY_WRITE`.
v0.9.0 has the same code. The fix walks all four levels of the page tables
and requires the user bit everywhere, and the writable bit everywhere for a
kernel write, so the same probe now gets `ERR_FAULT`; the leg that shows it
is part of the V0.10 gate. The site and the limitations page say so for
v0.9.0, which stays as released: a release is never rebuilt in place.

**V0.10: a userspace system, one audited step at a time.** The design was
argued out first — three independent plans, judged against each other, then
merged into one with fourteen binding rules — and the rule that shaped
everything else was that the kernel would stay non-preemptible. Background
programs would get the CPU only in bounded slices at a handful of places in
kernel code where nothing can be half-done: the prompt, the console's waits
for a job, every network poll, the desktop's idle loop. Before a single slice
ran, the ground was prepared so a slice could not corrupt anything: every
kernel lock counted (so a slice can refuse to run while one is held), output
from Ring 3 made line-atomic (so two programs can never splice a kernel
marker together), a program's direction and alignment-check flags cleared on
every entry to the kernel (a V0.9 defect: kernel code after a preemption ran
with the user's DF and AC), and the syscall stack given the guard page V0.9
had wrongly claimed it had. Each change carried its own negative control —
the same probe run against the kernel without the fix, to show the check can
fail.

The scheduling core then went in built, measured and switched off, and only
afterwards on. `sched` reports what the background got during each console
command and whether it starved; a paused `busy` starves it by design, a
resumed one leaves a gap of 50 ms. With slices in place the console could run
programs as ordinary processes, and the process model grew what an init
needs: parents, parent-only `wait`, a non-blocking `wait_nohang`, `sleep`,
orphans, `kill`. A sibling told to collect another program's child is
refused, and the probe that shows it fails the moment the parent check is
removed. `/sbin/init` came last: a Ring 3 program that reads its config with
the same host-tested parser the tests use, starts `tickd` and `flapd` with
exactly the capabilities the file names, supervises them, and reports each
event to the kernel, which checks everything it can know for itself. Killing
init ends its whole tree, and the kernel starts a new one.

Two more increments followed: a Ring 3 shell that borrows the console's
input from the kernel and gives it back, and Ring 3 applications on the live
desktop with click-to-focus and keys routed to the focused window. The last
one surfaced three old input bugs at once: a mouse-command acknowledgement
fed into the packet decoder framed the first real packet off by one (the V0.5
test only checked that some mouse line was printed, so it never noticed),
the PS/2 vertical axis was upside down relative to USB, and the input queue
allocated from the heap inside interrupt handlers. And writing `wait_nohang`
found a denial of service in every release so far: the kernel's copy into a
user buffer checked only that the page was mapped, so a program pointing a
capability-free call at its own code made the kernel fault in Ring 0. It was
disclosed for v0.9.0 the same evening and fixed on the branch.

**v0.10.0.** The branch was brought up to date with `main`, closed out
(architecture, security and threat model, the non-schedulable regions
measured rather than estimated) and given its release commit, which passed
the whole local gate — 50 QEMU legs, 456 host tests. GitHub still refused to
run jobs on the private repository, so, as for v0.9.0, the whole history was
scanned first and the repository made public for the release runs. The V0.10
legs had never run on GitHub's Linux runners; they passed there on the first
try, with the reproducibility job rebuilding both images from two clean
checkouts to the same digests. The CI-built bytes were boot-tested before the
site linked them — with a new check that the published image really starts
`/sbin/init` as pid 1 — and the site was read page by page for anything still
describing V0.9: the home page's layer model and the platform page's service
model were, and were corrected before production.

**The audit trail, reviewed against the programs it will record.** V0.11 puts
a proposing agent on top of the audit trail, so the V0.11 design went through
a security review before any agent code, and the review asked what the trail
itself would hold up to. Three defects came back, each present since V0.8.
The first was a verifier that cried wolf: `audit save` wrote only the current
boot's 64-record ring, under a head that covered every record ever made, and
recovery verified from the start of time — so on the published v0.10.0 image
a trail saved in the first boot verified, but the same trail saved again by
the second boot was `TAMPERED` on the third, though nobody had touched the
disk, and the first boot's records were gone from it. A mid-boot `audit
verify` did the same by re-running boot recovery. The old legs had only ever
saved once, in the first boot, with a handful of records: the one case that
worked. The second was that the trail was just a file: any program holding
`fs_write` could replace it, or delete an application's commit marker and
roll the application back. The third was that the kernel echoed file names
and file contents a program had chosen straight onto the console, so a name
with a line break in it printed a line of its own that read as a kernel
marker — the very thing V0.10 had stopped programs from printing directly.
All three are fixed on the V0.11 branch, each shown first on the released
image or on the fixed kernel with the fix switched off; v0.10.0 is disclosed
here and stays as released.

## 2026-09-14 — V0.11: an agent with intelligence and no authority (in development)

The rule V0.11 was designed around (ADR-0024) is that the agent may know
things and suggest things, and may never do anything. Everything it hands the
kernel is treated as a claim, and the kernel checks each claim against what
it knows itself. The only path from a suggestion to an action goes through
the console, and each action the kernel carries out is verified afterwards
and undone if the check fails. The layer was built bottom-up in ten steps.
Each step got its own QEMU leg and at least one control: the step's key check
disabled, the image rebuilt, the leg shown failing, the check restored.

**A model measured honestly.** The diagnostic model is deliberately small:
three integer yes/no detectors (a failed service, a paused scheduler, a burst
of denials) over sixteen features of a read-only system view, trained at build
time by an averaged perceptron, with no floating point anywhere. Its training
data is synthetic, generated from ranges written in `ai/scenarios.txt`,
because there is no fleet of real systems to learn from. The build trains it
and refuses to continue if the result does not match the digest pinned in the
repository; the kernel checks that digest again at boot. On held-out examples
it is exactly right every time, and a one-rule baseline is exactly right every
time too. That second number is the honest one. The scenarios define each
condition by essentially one feature, so the accuracy shows the designed
cases are separable. It does not show that the model can diagnose a real
machine. The requirements and the site say that rather than quote a
perfect score.

**The pieces, each with less authority than the last.** The kernel serves a
fixed 232-byte view of its own tables: counts, allow-listed service rows and
scheduler state, with no paths, arguments or payloads. Only a process the
console started with a console-only capability gets it: init cannot grant it,
delegation drops it and `/etc/init.conf` may not name it. A Ring 3 service,
`inferd`, runs the model under `/sbin/init`. It answers over IPC, which has no
sender identity, so its answers are claims too. The agent is a deterministic
Ring 3 program: it reads the view, asks `inferd`, looks up a fixed runbook
for each condition and prints a diagnosis. The leg builds three real
situations and requires each exact set of conditions. When the agent
proposes an action, the kernel runs its checks in order, and the first one
that fails names the refusal. The model must be the shipped one. The view
must be the last one served to that same process, and recent. The kernel
recomputes the condition from those exact bytes with its own copy of the
model. The action must apply to the system as the kernel sees it now. Ten
adversarial proposals are each refused for exactly their own reason. With
the recomputation switched off, a false diagnosis is still refused, but only
by the later applicability check, and its own refusal disappears. That
proves the check is what refuses it, not luck.

**Verification that measures something.** `approve` re-checks the proposal
against fresh facts, runs the action in kernel code, verifies it and rolls
it back if the check fails. The first action, resuming a paused scheduler,
could not be verified by "other processes made progress": the console's job
waits already give background processes slices while the scheduler is paused.
So verification uses the busy-point measurement, which a paused scheduler
fails. The control makes the executor do nothing and shows the verification
fail and the rollback run. The second action, retrying a failed service,
could not simply start the program: that would take the service's row away
from its supervisor. Instead the kernel posts a command to a one-slot mailbox
that only the live init can read and acknowledge. The mailbox is cleared if
init dies, so a command never reaches its successor. Verification is
event-driven: a watch trips on any failure or restart report for that
service inside the window, and rolls it back at once by stopping the service
and killing the instance.

Building the retry path found two ordering bugs in new code. Posting the
rollback's `stop` erased the retry's acknowledgement before the kernel had
read it, so a failing retry looked like one init had never answered. And the
watch was armed only after the command was posted, which left a gap where
init's report could arrive unwatched. Acknowledgements are now matched by
sequence number and never cleared by a post, and the watch is armed first.
The fixture that exercises the success path, `flakyd`, fails while the
system is under three seconds old. Its first run succeeded too early because
the leg waited in wall-clock time, and under QEMU's emulation guest time
runs slower than wall time. The leg now waits in guest time.

**One more console forgery.** Reviewing the approval path as an attacker,
the question was what the operator actually sees before typing
`approve`. V0.10 had stopped programs from printing the kernel's marker
prefix, but their escape sequences and carriage returns still reached the
terminal. A program could move the cursor up and rewrite the preview the
kernel had just printed. Process output now shows every control character
except the line feed and the tab as an escape (`\x1b`, `\x0d`). The leg's
probe tries it, and the control shows the raw sequence reaching the log
when the escaping is removed (OUT11-001).

**The review that did not stop at V0.11's own code.** Before the release,
six independent reviewers each took one area of everything V0.11 changed
and tried to break it. Every finding then went to a second reviewer told
to refute it, and nine survived. The two that mattered most were not in the
new code. V0.10 had stopped programs from printing the kernel's marker
prefix by rewriting it in each chunk of output — one chunk at a time. So a
program that wrote 250 filler bytes and then the marker put `[ITISY` at
the end of one 256-byte chunk and `OU:` at the start of the next. Neither
chunk contained the marker, and the serial port received it whole
(OUT11-002). And `ps` printed whatever path string a program had passed to
`spawn`. A component that a later `..` removed could carry a line break
and a marker, so the kernel itself printed the forgery (SEC11-002). Both are
in v0.10.0 and are disclosed for it. Now the buffer keeps back a tail that
could start a marker, and the kernel records the path of the file it
actually loaded.

The review also turned up a rollback that would kill whatever pid init
named. A deliberately compromised init, built for the experiment, got the
unfixed kernel to kill tickd. The kernel now kills only a live instance of
that service. Two tests turned out to be weaker than what their rows
claimed. One showed that each refusal reason appeared somewhere, not that
each run got its own. The harness grew an ordered require for it, and the
other became a kernel selftest. A documented residual also turned out to
be false: it said the next save would heal a tampered audit trail. It
doesn't, by design, and four new boots now prove the trail stays reported
until the operator removes it. Every one of these fixes was then shown
failing with the check removed, like the rest.

What V0.11 does not do is written down beside what it does. The model
diagnoses three conditions and knows nothing it was not designed to know. It
can propose only two actions. IPC is still unauthenticated, so a malicious
IPC holder can mislead a diagnosis, though it cannot get an unjustified
proposal filed. Proposals live only in memory. And because `flapd` fails by
design at boot, the shipped image never shows a system with nothing wrong
(host tests cover that case). Every V0.11 row is implemented and verified on
the development branch; the release comes next.
