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
  **Linux-only**: cargo's unstable bindeps unified crate features across
  the host/bare-metal boundary (bitflags built for x86_64-unknown-none with
  `std`), and `cargo tree` panics on such graphs. Root fix: removed
  artifact dependencies entirely; image-builder now runs
  `cargo build -p itisyou-kernel` as a subprocess (separate feature
  resolution, no unstable cargo features). Verified locally: clippy clean,
  images identical in role, full QEMU matrix re-run.
