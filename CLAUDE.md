# ITISYOU OS — project governance

This repository is governed by two binding documents. Read both before
modifying anything:

1. **`AGENT_OPERATING_RULES.md`** — the owner's operating rules for any coding
   agent (autonomy, quality bar, storage rules, git/secret/production safety,
   no fake completion, requirement traceability).
2. **`docs/IMPLEMENTATION_PLAN.md`** — the authoritative end-to-end execution
   specification for ITISYOU OS V0.1 and the public website.

Where they conflict, obey the stricter safety/verification requirement.

## Quick facts

- Kernel: Rust `no_std`, target `x86_64-unknown-none`, boots via the
  rust-osdev `bootloader` crate (BIOS + UEFI). **QEMU only** — never boot on
  physical hardware, never touch the host's boot chain or disks.
- Toolchain: pinned in `rust-toolchain.toml`. Caches live on `E:`
  (`RUSTUP_HOME=E:\toolchains\rustup`, `CARGO_HOME=E:\toolchains\cargo`) —
  never intentionally write project/build/cache data to `C:`.
- QEMU: `E:\tools\qemu\qemu-system-x86_64.exe`; OVMF firmware in
  `E:\tools\qemu\share\edk2-x86_64-code.fd`. Dot-source `scripts/env.ps1`
  for the canonical environment.
- One-command gate: `scripts/verify.ps1`. Boot/regression harness:
  `tools/qemu-runner` (asserts serial markers, absence of output is never
  success).
- Requirement states: `IMPLEMENTED + VERIFIED`, `BLOCKED`, `NOT APPLICABLE` —
  tracked in `docs/REQUIREMENTS.md`. There is no generic "done".
- Website: `website/` → deployed to `https://os.itisyou.app` via Cloudflare.
  Content must never outrun engineering evidence (`status/current.json` is
  the source of truth).
- Design source of truth: `design/stitch/` (preserved Stitch export +
  original ZIP). Do not modify the exported evidence.
- Run `scripts/secret-scan.ps1` before every push.
