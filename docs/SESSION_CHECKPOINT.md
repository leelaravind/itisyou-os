# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~02:45 Europe/London (session 1 in progress)
**Repository:** `E:\Project\itisyou-os` · branch `main` · remote: pending first push
**Milestone:** V0.1 Kernel Foundation — Phase 1 (repository & build skeleton)

## Environment (verified)

- Rust nightly-2026-09-01 + `x86_64-unknown-none` at `E:\toolchains\{rustup,cargo}`
- QEMU 11.1.0 at `E:\tools\qemu` (OVMF: `share\edk2-x86_64-code.fd`)
- GitHub CLI authenticated (`leelaravind`); Cloudflare auth not yet discovered
- Design ZIP preserved: `design/stitch_itisyou_os_architecture_portal.zip`
  (+ extracted `design/stitch/`, 13 screens + DESIGN.md)

## Current state

- Workspace authored: kernel (lib + interactive/selftest bins),
  kernel-core (host-tested contract), image-builder, qemu-runner, scripts,
  docs, status metadata.
- First build in progress (fix applied: bindeps enabled via
  `.cargo/config.toml`, not manifest `cargo-features`).

## Next actions (exact order)

1. Finish `cargo build -p image-builder -p qemu-runner`; run host tests.
2. `cargo run -p image-builder -- target/images` → 4 images + manifest.
3. `cargo run -p qemu-runner -- --image target/images/itisyou-kernel-selftest-bios.img --expect B010 --expect B020 --expect B030 --expect-selftest --label selftest-bios` (then UEFI with `--uefi`).
4. Commit checkpoint, create private GitHub repo `leelaravind/itisyou-os`, secret-scan, push.
5. Phase 2 kernel subsystems in dependency order (plan §20).

## Blockers

None hard. Cloudflare deployment auth is an open discovery item (CF-001) —
not blocking kernel/website build work.
