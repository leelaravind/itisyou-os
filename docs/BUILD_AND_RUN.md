# Build and run — ITISYOU OS V0.1

Reproducible environment, build, and QEMU instructions for a Windows host.
Every command here is the exact command used in verification.

## Prerequisites

| Tool | Version used | Location |
|---|---|---|
| Rust (rustup) | nightly-2026-08-01 (cargo 1.99.0-nightly), pinned in `rust-toolchain.toml` (newer nightlies break the bootloader's UEFI stage, rust-osdev/bootloader#579) | `RUSTUP_HOME=E:\toolchains\rustup`, `CARGO_HOME=E:\toolchains\cargo` |
| Target | `x86_64-unknown-none` + components rust-src, clippy, rustfmt, llvm-tools | via `rust-toolchain.toml` |
| QEMU | 11.1.0 (qemu.weilnetz.de w64 build) | `E:\tools\qemu\qemu-system-x86_64.exe` |
| OVMF (UEFI firmware) | edk2 build shipped with QEMU | `E:\tools\qemu\share\edk2-x86_64-code.fd` |
| Visual Studio Build Tools | VS2022 (MSVC linker, host tools only) | system |

Storage policy (operating rules §5): all toolchain caches and build output
live on `E:`; bulk scratch on `G:\claude-tmp` when that external drive is
attached and `E:\claude-tmp` otherwise (`$env:ITISYOU_SCRATCH`); nothing
project-related is intentionally written to `C:`.

`scripts/env.ps1` sets this environment; every project script dot-sources it.
Run `scripts/doctor.ps1` to validate the environment without installing
anything.

## Build

```powershell
scripts\build.ps1            # = cargo run -p image-builder -- target/images
```

This builds (via artifact dependencies) both kernel binaries for
`x86_64-unknown-none` and produces four bootable images plus a checksum
manifest in `target/images/`:

- `itisyou-kernel-{bios,uefi}.img` — interactive kernel
- `itisyou-kernel-selftest-{bios,uefi}.img` — in-kernel selftest suite
- `manifest.txt` — sizes + SHA-256 for release evidence

## Run in QEMU

```powershell
scripts\run-qemu.ps1                      # UEFI (OVMF), display window
scripts\run-qemu.ps1 -Firmware bios       # legacy BIOS boot
scripts\run-qemu.ps1 -Headless            # serial only
scripts\run-qemu.ps1 -Selftest -Headless  # run the selftest kernel
```

Serial output appears on stdio. The kernel emits machine-readable boot-stage
markers (`[ITISYOU:B010] …`); the grammar is defined in
`crates/kernel-core/src/marker.rs`.

QEMU safety: only project-generated disposable images are ever attached; no
host disk passthrough, no host filesystem sharing (plan §11.2).

## Test

```powershell
scripts\test.ps1     # host unit tests + 4-way QEMU boot/selftest matrix
scripts\verify.ps1   # full gate: doctor, fmt, clippy, tests, website, secret scan
```

QEMU evidence (serial logs + JSON results) is written to `artifacts/qemu/`
(git-ignored; summarized evidence lands in `status/`).
