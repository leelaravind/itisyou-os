# Requirement traceability matrix — ITISYOU OS V0.1

States: `IMPLEMENTED+VERIFIED` (with evidence) · `IN PROGRESS` (work ongoing
this session) · `PLANNED` (not started) · `BLOCKED` (reason + required input)
· `NOT APPLICABLE` (reason). Final reporting collapses everything to the three
terminal states required by the implementation plan §2.

Evidence conventions: `verify.ps1` stage names, `artifacts/qemu/*.result.json`
labels, commit SHAs, CI run links, or file paths.

## Governance & environment

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| GOV-001 | Agent rules loaded and referenced | IMPLEMENTED+VERIFIED | `AGENT_OPERATING_RULES.md`, `CLAUDE.md`, `docs/IMPLEMENTATION_PLAN.md` in repo |
| GOV-002 | No intentional project writes to C:/D:/F: | IN PROGRESS | `scripts/env.ps1` + `doctor.ps1` storage checks; RUSTUP_HOME/CARGO_HOME on E:; QEMU on E:; scratch on G: |
| GIT-001 | Recoverable private repo created/reused | IN PROGRESS | local git initialized; GitHub repo pending first push |
| GIT-002 | Secret scan before pushes | IN PROGRESS | `scripts/secret-scan.ps1` |
| ENV-001 | Toolchain verified | IN PROGRESS | `scripts/doctor.ps1`; nightly-2026-09-01 + x86_64-unknown-none on E:; QEMU 11.1.0 at E:\tools\qemu |

## Kernel

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| KRN-001 | x86_64 independent kernel builds | IMPLEMENTED+VERIFIED | `cargo run -p image-builder` produces BIOS images + SHA-256 manifest (2026-09-02, `target/images/manifest.txt`) |
| BOOT-001 | QEMU reaches kernel entry (B010) | IMPLEMENTED+VERIFIED | `boot-smoke-bios` outcome=Success, stages B010/B020/B030 observed (artifacts/qemu/boot-smoke-bios.result.json) |
| BOOT-002 | Serial logging works (B020) | IMPLEMENTED+VERIFIED | same + `selftest-bios` outcome=Success exit=33, pass=2 fail=0 |
| BOOT-003 | UEFI (OVMF) boot path | BLOCKED (upstream) | bootloader-x86_64-uefi 0.11.17 fails to link on current nightlies: `rust-lld: undefined symbol: wcslen` — rust-osdev/bootloader#579 (open, no fix). BIOS path is the verified boot path; probe of older nightly in progress. Re-enable via image-builder `uefi` feature when fixed |
| MEM-001 | Physical memory manager works | PLANNED | unit + QEMU selftests |
| MEM-002 | Virtual memory abstraction works | PLANNED | mapping tests |
| MEM-003 | Kernel heap works | PLANNED | allocation tests |
| INT-001 | Exceptions/IDT installed | PLANNED | controlled exception test |
| TIM-001 | Timer/ticks work | PLANNED | monotonic tick evidence |
| TASK-001 | Scheduler runs multiple tasks | PLANNED | deterministic task output tests |
| FS-001 | VFS/initramfs mounts | PLANNED | ls/cat integration test |
| SH-001 | Shell commands execute robustly | PLANNED | serial interaction tests |
| DIAG-001 | Panic produces useful serial evidence | IN PROGRESS | panic handler emits `[ITISYOU:PANIC]`; negative test pending |
| USR-001 | Ring 3/userspace hello works (stretch) | PLANNED | QEMU proof required; must not destabilize hard target |
| ABI-001 | Syscall ABI defined/tested (stretch) | PLANNED | tests/docs |

## Testing & verification

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| TEST-001 | One-command verification gate exists | IN PROGRESS | `scripts/verify.ps1` |
| TEST-002 | QEMU timeout/failure classification works | IN PROGRESS | `tools/qemu-runner` classify(); negative tests pending |
| SEC-001 | Unsafe inventory exists | PLANNED | `docs/UNSAFE_INVENTORY.md` |
| SEC-002 | No host disk passthrough | IN PROGRESS | qemu-runner/run-qemu.ps1 only reference project-generated images |
| REL-001 | Build manifest/checksums produced | IN PROGRESS | `image-builder` writes manifest.txt with SHA-256 |

## Documentation

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| DOC-001 | Required docs maintained | IN PROGRESS | docs/ tree; development story + ADRs started |

## Website & deployment

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| WEB-001 | Website implements approved Stitch design | PLANNED | `design/stitch/` preserved as source of truth |
| WEB-002 | Status is evidence-backed | PLANNED | `status/current.json` generated from verification |
| WEB-003 | Responsive/accessibility gate | PLANNED | automated + browser evidence |
| CF-001 | os.itisyou.app deployed & verified | PLANNED | production browser verification required; Cloudflare auth to be discovered |
| CI-001 | CI checks repo on push/PR | PLANNED | GitHub Actions |

Update this file as evidence lands. Never delete an original requirement
because it is difficult (plan §19).
