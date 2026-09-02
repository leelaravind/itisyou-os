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
| USR-001 | Ring 3 userspace program runs | IMPLEMENTED+VERIFIED | (V0.2) `/bin/init` loaded from initramfs ELF, entered via iretq, emits RING3-HELLO…RING3-DONE via write syscalls, exits 0 — selftest `usr_init_clean_exit`; CPL=3 hardware-proven by `#GP cs_rpl=3` on `hlt` and `#PF error=USER_MODE` on kernel-half read |
| ABI-001 | Syscall ABI defined/tested | IMPLEMENTED+VERIFIED | (V0.2) ABI documented in kernel/src/syscall.rs + ADR-0004, mirrored by user/ulib; write/exit/yield/getpid round-trips + ERR_NOSYS + pointer-validation errors all exercised from Ring 3 (RING3-EINVAL-OK, RING3-BADPTR-OK, RING3-UNMAPPED-OK, RING3-YIELD-OK) |

## V0.2 — Userspace Foundation

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| USR-002 | ELF64 parser/loader with strict validation | IMPLEMENTED+VERIFIED | 11 host tests (kernel-core::elf, every malformed class) + in-QEMU `usr_elf_reject_malformed` (/bin/broken) |
| USR-003 | W^X + user-window + overlap load policy | IMPLEMENTED+VERIFIED | `usr_elf_reject_wx` (/bin/wx-test fixture); loader rejects out-of-window and already-mapped pages by construction |
| USR-004 | Kernel/user memory separation | IMPLEMENTED+VERIFIED | user pages USER_ACCESSIBLE in [1 MiB, 512 GiB) only; kernel mappings supervisor-only; `usr_pf_contained` proves MMU blocks user reads of kernel half (CR2=0xffff8000dead0000, USER_MODE bit) |
| USR-005 | Process lifecycle + clean termination + teardown | IMPLEMENTED+VERIFIED | pids assigned; exit(0) path (`user_exit pid=… code=0`); `usr_reload_after_teardown` re-loads at the same addresses after full unmap+frame-free |
| USR-006 | Crash isolation (user fault ≠ kernel panic) | IMPLEMENTED+VERIFIED | `usr_gp_contained` (#GP on privileged `hlt` at CPL=3) + `usr_pf_contained`; `usr_kernel_alive_after_faults` (allocator + timer functional afterwards) |
| USR-007 | Invalid syscall + invalid pointer handling | IMPLEMENTED+VERIFIED | ERR_NOSYS for nr=999; ERR_FAULT for kernel-half and unmapped-in-window pointers — validated before any dereference (RING3-EINVAL/BADPTR/UNMAPPED-OK) |
| USR-008 | Scheduler integration (yield from Ring 3) | IMPLEMENTED+VERIFIED | yield syscall parks in the kernel scheduler and returns to Ring 3 (RING3-YIELD-OK) |
| USR-009 | Userspace stdout path | IMPLEMENTED+VERIFIED | write(fd=1) → serial with UTF-8 sanitization; all RING3-* lines are user-produced output |
| USR-010 | Per-process page tables / concurrent user processes | IMPLEMENTED+VERIFIED | (V0.3) see PROC/ASPACE rows below |

## V0.3 — Process Isolation + Storage Foundation

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| ASPACE-001 | Per-process page tables (dedicated address space) | IMPLEMENTED+VERIFIED | ADR-0005; `aspace_distinct_frames` — same user vaddr → different physical frames in two processes |
| ASPACE-002 | Shared/protected kernel mappings | IMPLEMENTED+VERIFIED | L4 entries 1..512 shared from boot table (supervisor-only); `aspace_kernel_table_untouched` (user vaddr unmapped in kernel table) |
| ASPACE-003 | Safe CR3 / address-space switching | IMPLEMENTED+VERIFIED | `activate_l4` around each quantum; boot L4 restored on return; concurrent run of 3 processes stable |
| ASPACE-004 | Complete teardown without leaks | IMPLEMENTED+VERIFIED | `aspace_teardown_no_leaks`, `usr_run_no_frame_leaks`, `proc_concurrent_no_leaks` — frame-exact accounting across load/run/teardown |
| ASPACE-005 | Cross-process memory isolation | IMPLEMENTED+VERIFIED | `aspace_cross_process_isolation` — sentinel written to process A's frame is absent from process B's same-vaddr frame |
| PROC-001 | Concurrent Ring 3 processes + scheduler | IMPLEMENTED+VERIFIED | ADR-0006; `proc_concurrent_spawn_wait_ipc` — parent + 2 children interleave (both print before either exits), run-loop round-robin |
| PROC-002 | PID management + process states/lifecycle | IMPLEMENTED+VERIFIED | monotonic pids; Runnable/Blocked/Exited/Faulted; `user_exit`/`user_fault` markers with pid |
| PROC-003 | spawn / wait / exit semantics | IMPLEMENTED+VERIFIED | spawn(path)→child pid; wait blocks until zombie, returns status (RING3-PARENT-WAIT-OK: both children 7); exit contained |
| PROC-004 | User-process fault containment (concurrent) | IMPLEMENTED+VERIFIED | `usr_gp_contained`, `usr_pf_contained`, `usr_kernel_alive_after_faults` still green with per-process spaces |
| PROC-005 | Timer-driven preemption | NOT APPLICABLE (deferred) | cooperative scheduling this milestone; preemptive user scheduling needs full ISR trap-frame save/restore — V0.4 (KNOWN_LIMITATIONS, ADR-0006) |
| SYS-001 | syscall argument + user-buffer validation | IMPLEMENTED+VERIFIED | `validate_user_range`/`copy_from_user`/`copy_to_user` (active-CR3, window-bounded, per-page); RING3-BADPTR/UNMAPPED-OK; spawn path bounded to 128 B |
| SYS-002 | Invalid/hostile syscall handling | IMPLEMENTED+VERIFIED | ERR_NOSYS (RING3-EINVAL-OK), ERR_FAULT, ERR_BADF, ERR_NOENT, ERR_2BIG, ERR_AGAIN, ERR_INVAL |
| IPC-001 | Minimal IPC primitive | IMPLEMENTED+VERIFIED | ADR-0007; bounded kernel channels; RING3-PARENT-IPC-OK (send/recv round-trip + ERR_AGAIN on empty) |
| PCI-001 | PCI enumeration hardening | IMPLEMENTED+VERIFIED | `device::pci` + host-tested `kernel_core::pci` (4 tests); `pci_enumerated` (7 devices), `pci_found_nvme` |
| BLK-001 | Block-device abstraction + error handling | IMPLEMENTED+VERIFIED | `BlockDevice` trait + RamDisk; `blk_ramdisk_read`, `blk_out_of_range_rejected`, `blk_bad_buffer_rejected` |
| NVME-001 | Read-only NVMe driver (real block I/O) | IMPLEMENTED+VERIFIED | ADR-0008; `nvme_init` (2048 blocks identified), `nvme_read_block0`, `nvme_disk_magic` (LBA 0 magic read back), `nvme_out_of_range_rejected` |
| STORE-001 | No physical-disk risk | IMPLEMENTED+VERIFIED | QEMU attaches only a generated disposable raw disk (runner `--nvme`); no passthrough anywhere |

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
| DOC-001 | Required docs maintained | IMPLEMENTED+VERIFIED | full set current and updated during (not after) implementation: ARCHITECTURE, IMPLEMENTATION_PLAN (verbatim copy), REQUIREMENTS, THREAT_MODEL, SECURITY_MODEL, TESTING, BUILD_AND_RUN, DEPLOYMENT, RECOVERY, KNOWN_LIMITATIONS, ROADMAP, DEVELOPMENT_STORY, SESSION_CHECKPOINT, UNSAFE_INVENTORY, ADRs 0001–0003 |

## Website & deployment

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| WEB-001 | Website implements approved Stitch design | IMPLEMENTED+VERIFIED | 20 routes from the preserved `design/stitch/` export; browser-reviewed on staging + production (home, build, 404 screenshots; design tokens per DESIGN.md); deviations recorded in `website/DESIGN_DEVIATIONS.md` |
| WEB-002 | Status is evidence-backed | IMPLEMENTED+VERIFIED | `sync-status.mjs` copies + vocabulary-validates `status/current.json` on every build (build fails on drift); /build states the truth policy; commit field renders "—" until stamped |
| WEB-003 | Responsive/accessibility gate | IMPLEMENTED+VERIFIED (with noted limit) | astro check 0 errors; semantic landmarks/skip link/focus states/contrast computed ≥4.5:1/reduced-motion in build output; zero client JS; live mobile-viewport screenshot not capturable (maximized-window environment prevented browser resize) — responsive breakpoints verified in built CSS |
| CF-001 | os.itisyou.app deployed & verified | IMPLEMENTED+VERIFIED | Worker os-itisyou-app-production + custom domain; verified live: HTTP 200 + TLS, 11 routes 200, /nope → 404, strict CSP/nosniff/DENY headers observed, browser journey with zero console errors (staging identically verified first) |
| CI-001 | CI checks repo on push/PR | IMPLEMENTED+VERIFIED | run 33598999034 (commit 9a1d847) **success**: fmt, split clippy, 41 host tests, image build, all 6 QEMU legs (BIOS+UEFI smoke, selftests, shell interaction, intentional panic), website check+build, gitleaks — all green on ubuntu-24.04 |

Update this file as evidence lands. Never delete an original requirement
because it is difficult (plan §19).
