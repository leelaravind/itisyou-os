# Threat model — ITISYOU OS V0.1

V0.1 is a pre-alpha research kernel that runs **only inside QEMU on the
developer's machine**. The threat model is therefore scoped to development
reality, and grows with each milestone (plan §11).

## Assets

- The developer's host machine (Windows laptop): its disks, boot chain,
  firmware, and data. **Highest-value asset; the OS must never touch it.**
- The repository: source integrity, commit history, requirement evidence.
- Credentials used by tooling (GitHub, Cloudflare) — never enter the repo.
- The public website's truthfulness (a false capability claim is an
  integrity failure).

## Adversary/failure assumptions for V0.1

There is no multi-user or network exposure yet; realistic risks are:

| Risk | Vector | Mitigation |
|---|---|---|
| Host damage from OS testing | misconfigured VM (disk passthrough, raw device writes) | QEMU launches only project-generated images; no passthrough flags anywhere in scripts/harness; physical-hardware boot is out of scope (plan §27) |
| Kernel bug corrupting its own state silently | logic errors, bad `unsafe` | panic-on-invariant-violation; boot-stage markers; selftests; unsafe inventory |
| Malformed boot data | firmware/bootloader handoff anomalies | validate memory map/pointers before use; reject overlapping regions (MEM-001 tests) |
| Malformed initramfs | corrupt/hostile archive bytes | strict bounds-checked parsing with negative-case tests (planned with FS-001) |
| Supply-chain drift | dependencies with unexpected code | small pinned dependency set, `Cargo.lock` committed, rationale per kernel dependency |
| Secret leakage | tokens in commits/logs/CI | `scripts/secret-scan.ps1` before pushes; env-var/platform secret stores only |
| Fake completion | claims without evidence | requirement matrix + machine-readable QEMU evidence; website generated from `status/current.json` |

## Explicit non-threats for V0.1

Remote attackers (no network stack), malicious local users (single-developer
VM), physical attacks, and side channels are out of scope until the relevant
subsystems exist. Each later milestone (userspace, storage, networking) must
extend this document before shipping the feature.
