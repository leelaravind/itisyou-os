# Build and run — ITISYOU OS

Reproducible environment, build, and QEMU instructions for a Windows host
(the project's primary host) and, in the last section, a Linux host.
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

## Linux host

The scripts above are PowerShell and set Windows paths. On Linux the
workflow file is the script: `scripts/run-ci-local.py` executes the `run:`
steps of `.github/workflows/ci.yml` in order, exactly as written, so there is
no second copy of the matrix to keep in step. Verified on Ubuntu 24.04
(2026-11-14; results in `docs/LOCAL_LINUX_RUN.md`).

Prerequisites:

```sh
# Rust: rustup; the pinned nightly, components and target install themselves
# from rust-toolchain.toml on the first cargo command.
sudo apt-get update
sudo apt-get install -y --no-install-recommends qemu-system-x86 ovmf python3-yaml
```

Ubuntu 24.04 ships QEMU 8.2.2 and OVMF at `/usr/share/OVMF/OVMF_CODE_4M.fd`
(the path the workflow passes with `--firmware`). KVM is not needed; the
matrix runs under TCG.

```sh
python3 scripts/run-ci-local.py --list              # every step, with its index
python3 scripts/run-ci-local.py                     # the whole kernel job
python3 scripts/run-ci-local.py --from 5 --to 10    # fmt, clippy, host tests, images
python3 scripts/run-ci-local.py --from 13           # the QEMU matrix only
python3 scripts/run-ci-local.py --only "TCP stream" # one leg, by name
python3 scripts/run-ci-local.py --job website       # npm ci + site verify
```

Each step's output goes to `target/ci-local/<job>/<index>.log`, the verdicts
to `summary.tsv` beside them, and the QEMU evidence to `artifacts/qemu/` as on
Windows. The whole kernel job takes about 20 minutes on four cores. Run one
gate at a time: the legs share disk-image names under `artifacts/`.

Two things differ from the Windows gate and are not hidden by the script:

- **It is not a GitHub Actions run.** A green local run does not satisfy a
  requirement whose evidence is a CI run id (V1-REL-001, V1-REL-002,
  V1-REP-004).
- **Images built on Linux have different digests from images built on
  Windows** (path separators in panic locations), so a Linux build never
  reproduces the published Windows-built bytes. Two Linux builds do
  reproduce each other.
