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
