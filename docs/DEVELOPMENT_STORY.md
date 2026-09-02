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
