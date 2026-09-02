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
| GOV-002 | No intentional project writes to C:/D:/F: | IMPLEMENTED+VERIFIED | `scripts/env.ps1` routes RUSTUP_HOME/CARGO_HOME/TEMP/TMP to E:/G:; doctor checks enforce it; npm cache redirected per-command; QEMU extracted to E: without elevation |
| GIT-001 | Recoverable private repo created/reused | IMPLEMENTED+VERIFIED | https://github.com/leelaravind/itisyou-os (private); checkpoint commits pushed (caae006, a0d5d33, 26d6948, …) |
| GIT-002 | Secret scan before pushes | IMPLEMENTED+VERIFIED | `scripts/secret-scan.ps1` clean before each push (109 → 144 files); gitleaks job in CI |
| ENV-001 | Toolchain verified | IMPLEMENTED+VERIFIED | doctor.ps1 all-OK; nightly-2026-08-01 + x86_64-unknown-none on E:; QEMU 11.1.0 + OVMF at E:\tools\qemu; VS2022 MSVC host linker |

## Kernel

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| KRN-001 | x86_64 independent kernel builds | IMPLEMENTED+VERIFIED | `cargo run -p image-builder` produces BIOS images + SHA-256 manifest (2026-09-02, `target/images/manifest.txt`) |
| BOOT-001 | QEMU reaches kernel entry (B010) | IMPLEMENTED+VERIFIED | `boot-smoke-bios` outcome=Success, stages B010/B020/B030 observed (artifacts/qemu/boot-smoke-bios.result.json) |
| BOOT-002 | Serial logging works (B020) | IMPLEMENTED+VERIFIED | same + `selftest-bios` outcome=Success exit=33, pass=2 fail=0 |
| BOOT-003 | UEFI (OVMF) boot path | IMPLEMENTED+VERIFIED | recovered by pinning nightly-2026-08-01 (newest nightly unaffected by rust-osdev/bootloader#579); boot-smoke-uefi + selftest-uefi outcome=Success |
| MEM-001 | Physical memory manager works | IMPLEMENTED+VERIFIED | 15 host tests (memmap+bitmap incl. malformed maps, double-free, exhaustion) + QEMU selftests `pmm_*` pass on BIOS & UEFI (selftest-{bios,uefi}.result.json pass=27 fail=0) |
| MEM-002 | Virtual memory abstraction works | IMPLEMENTED+VERIFIED | QEMU selftests `paging_map_translate`, `paging_write_read`, `paging_unmap_clears`, `paging_wx_rejected` |
| MEM-003 | Kernel heap works | IMPLEMENTED+VERIFIED | QEMU selftests `heap_box`, `heap_vec_growth`, `heap_large_alloc`, `heap_alignment_u128`, `heap_stats_sane` |
| INT-001 | Exceptions/IDT installed | IMPLEMENTED+VERIFIED | QEMU selftest `idt_breakpoint_resumes` (int3 handled + resumed); double-fault IST installed; fault handlers panic with vector/error/address |
| TIM-001 | Timer/ticks work | IMPLEMENTED+VERIFIED | QEMU selftests `timer_ticks_advance`, `timer_monotonic` (PIT @100 Hz) |
| TASK-001 | Scheduler runs multiple tasks | IMPLEMENTED+VERIFIED | QEMU selftests `sched_spawn`, `sched_all_finished`, `sched_both_completed`, `sched_interleave_deterministic` (= "ababab") |
| FS-001 | VFS/initramfs mounts | IMPLEMENTED+VERIFIED | 7 host tar tests (incl. truncated/corrupt archives) + QEMU selftests `vfs_ls_root`, `vfs_cat_version`, `vfs_missing_path`, `vfs_traversal_rejected`, `vfs_relative_normalized`, `vfs_dir_not_file` |
| SH-001 | Shell commands execute robustly | IMPLEMENTED+VERIFIED | shell-test-bios: 12 commands driven over serial, 9 required outputs matched, unknown-command + bounded-args paths exercised, stages through B150, exit 33 |
| DIAG-001 | Panic produces useful serial evidence | IMPLEMENTED+VERIFIED | panic-test-bios outcome=Success: intentional panic emits single-line `[ITISYOU:PANIC] <msg> at <file:line>` |
| USR-001 | Ring 3/userspace hello works (stretch) | PLANNED | QEMU proof required; must not destabilize hard target |
| ABI-001 | Syscall ABI defined/tested (stretch) | PLANNED | tests/docs |

## Testing & verification

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| TEST-001 | One-command verification gate exists | IMPLEMENTED+VERIFIED | `scripts/verify.ps1` full run 2026-09-02: doctor OK, fmt OK, clippy OK, 41 host tests, 6/6 QEMU legs Success, website 0 errors, secret scan clean → "VERIFY: OK" |
| TEST-002 | QEMU timeout/failure classification works | IMPLEMENTED+VERIFIED | classifications observed operating correctly during real debugging: Timeout (interactive halt), Panic (UEFI TooManyRegions), MissingMarkers (FIFO stall), Success; negative leg panic-test-bios green |
| SEC-001 | Unsafe inventory exists | IMPLEMENTED+VERIFIED | `docs/UNSAFE_INVENTORY.md` — 12 documented unsafe contracts |
| SEC-002 | No host disk passthrough | IMPLEMENTED+VERIFIED | qemu-runner `build_command` + run-qemu.ps1 attach only `target/images/*.img`; no passthrough flags anywhere |
| REL-001 | Build manifest/checksums produced | IMPLEMENTED+VERIFIED | `target/images/manifest.txt`: 6 images with sizes + SHA-256 (host test covers SHA-256 vectors) |

## Documentation

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| DOC-001 | Required docs maintained | IN PROGRESS | docs/ tree; development story + ADRs started |

## Website & deployment

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| WEB-001 | Website implements approved Stitch design | IMPLEMENTED+VERIFIED | 20 routes from the preserved `design/stitch/` export; browser-reviewed on staging + production (home, build, 404 screenshots; design tokens per DESIGN.md); deviations recorded in `website/DESIGN_DEVIATIONS.md` |
| WEB-002 | Status is evidence-backed | IMPLEMENTED+VERIFIED | `sync-status.mjs` copies + vocabulary-validates `status/current.json` on every build (build fails on drift); /build states the truth policy; commit field renders "—" until stamped |
| WEB-003 | Responsive/accessibility gate | IMPLEMENTED+VERIFIED (with noted limit) | astro check 0 errors; semantic landmarks/skip link/focus states/contrast computed ≥4.5:1/reduced-motion in build output; zero client JS; live mobile-viewport screenshot not capturable (maximized-window environment prevented browser resize) — responsive breakpoints verified in built CSS |
| CF-001 | os.itisyou.app deployed & verified | IMPLEMENTED+VERIFIED | Worker os-itisyou-app-production + custom domain; verified live: HTTP 200 + TLS, 11 routes 200, /nope → 404, strict CSP/nosniff/DENY headers observed, browser journey with zero console errors (staging identically verified first) |
| CI-001 | CI checks repo on push/PR | IN PROGRESS | run 33598093709: website + secret-scan jobs green; kernel job failed at clippy (Linux-only bindeps feature-unification) — fixed by removing artifact deps (subprocess kernel build); re-verification on next push |

Update this file as evidence lands. Never delete an original requirement
because it is difficult (plan §19).
