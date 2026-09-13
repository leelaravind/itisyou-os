# Requirement traceability matrix — ITISYOU OS

States: `IMPLEMENTED+VERIFIED` (with evidence) · `IN PROGRESS` (work ongoing
this session) · `PLANNED` (not started) · `BLOCKED` (reason + required input)
· `NOT APPLICABLE` (reason). Only the first, `BLOCKED` and `NOT APPLICABLE`
are terminal (implementation plan §2); there is no `DONE` or `NOT DONE`
state. `website/scripts/check-consistency.mjs` enforces the vocabulary on
every build, and its `--release` form refuses any non-terminal row before a
version tag is created.

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
| STORE-001 | No physical-disk risk | IMPLEMENTED+VERIFIED | QEMU attaches only generated disposable raw disks (runner `--nvme`/`--nvme-persist`); no passthrough anywhere |

## V0.4 — Preemptive Multitasking + Persistent Storage

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| PRE-001 | Timer-driven preemption of Ring 3 | IMPLEMENTED+VERIFIED | ADR-0009; naked timer ISR saves full trap frame; `preempt_actually_occurred` (preemption count > 0) |
| PRE-002 | Non-yielding process cannot monopolize | IMPLEMENTED+VERIFIED | `preempt_no_monopoly` — an infinite no-syscall spinner is preempted; a co-scheduled finite process still completes; spinner still Runnable, then reaped |
| PRE-003 | Registers preserved across preemption | IMPLEMENTED+VERIFIED | `preempt_two_cpu_bound_progress` — spin-finite verifies its own sum + stack sentinel; corruption exits nonzero, both exit 0 |
| PRE-004 | Address spaces isolated across preemption | IMPLEMENTED+VERIFIED | spin-finite's per-process stack sentinel holds across dozens of preemptions (part of PRE-003); distinct CR3 per quantum |
| PRE-005 | Faults contained + exit/wait work under preemption | IMPLEMENTED+VERIFIED | `preempt_coexists_with_cooperative` (yield/wait/IPC still works); V0.2 fault-containment tests still green under the preemptive timer |
| PRE-006 | No frame leaks / lost processes across switching | IMPLEMENTED+VERIFIED | `preempt_no_frame_leaks`, `preempt_no_lost_processes` (frame-exact + live-slot accounting) |
| PRE-007 | Cooperative V0.1–V0.3 scheduling regressions green | IMPLEMENTED+VERIFIED | full selftest pass=63 fail=0 BIOS+UEFI incl. all sched_*/proc_* tests |
| NVW-001 | NVMe write → flush → read round-trip | IMPLEMENTED+VERIFIED | `nvme_write_flush_read_roundtrip` (write scratch LBA, flush, read back identical) |
| NVW-002 | Out-of-range write rejected | IMPLEMENTED+VERIFIED | `nvme_write_out_of_range_rejected` (rejected before any I/O) |
| FS-ITFS-001 | ITFS format/create/read/list | IMPLEMENTED+VERIFIED | `fs_format_create_read_list`, `fs_missing_file_rejected`, `fs_remount_reads_committed` over the real NVMe device |
| FS-ITFS-002 | Strict on-disk metadata validation | IMPLEMENTED+VERIFIED | 8 host `kernel_core::itfs` tests (bad magic/CRC/inconsistent) + `fs_both_superblocks_corrupt_rejected` |
| FS-ITFS-003 | Crash consistency (torn superblock) | IMPLEMENTED+VERIFIED | `fs_crash_consistency_torn_superblock` — corrupt the newest superblock slot; mount recovers the older consistent slot |
| FS-ITFS-004 | Reboot persistence through the block layer | IMPLEMENTED+VERIFIED | two-boot UEFI test on one disposable disk: `FS-PERSIST-WROTE` (boot 1) then `FS-PERSIST-VERIFIED` (boot 2, fresh guest) — the hard V0.4 acceptance target |
| BLK-002 | Block abstraction works across devices | IMPLEMENTED+VERIFIED | same `BlockDevice` trait + ITFS logic run over NVMe (device) and RamDisk (`blk_*` tests) |
| STORE-2ND | Second block-device backend (AHCI/virtio-blk) | NOT APPLICABLE (deferred) | goal item 5 is conditional ("if technically appropriate"); the `BlockDevice` trait is ready and ITFS is device-agnostic (proven over NVMe + RamDisk). Deferred to V0.5 to avoid rushing a second driver; no higher logic is per-driver |
| CI-V04 | CI green with V0.4 QEMU coverage | IMPLEMENTED+VERIFIED | run 33619289190 success (kernel build, split clippy, 63 host tests, images, QEMU legs incl. preemption + NVMe + FS + two-boot persistence, website, gitleaks) on ubuntu-24.04 |
| WEB-V04 | os.itisyou.app reflects V0.4 truthfully | IMPLEMENTED+VERIFIED | live at v0.4.0-dev commit b1af235, milestone "V0.4 — Preemptive Multitasking + Persistent Storage", verified modules (preemption/filesystem/nvme write), browser+HTTP verified, zero console errors |
| CI-V03 | CI green with V0.3 QEMU coverage | IMPLEMENTED+VERIFIED | run 33609090873 success (kernel build, split clippy, 55 host tests, images, 6 QEMU legs incl. NVMe+concurrency asserts, website, gitleaks) on ubuntu-24.04 |
| WEB-V03 | os.itisyou.app reflects V0.3 truthfully | IMPLEMENTED+VERIFIED | live at v0.3.0-dev commit d83942b, milestone "V0.3 — Process Isolation + Storage", verified modules (per-process/ipc/pci/block/nvme), browser+HTTP verified, zero console errors, stale claims removed |

## V0.5 — Graphics + Input + Basic Desktop/Compositor

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| GFX-001 | OS boots into a real graphical environment in QEMU | IMPLEMENTED+VERIFIED | ADR-0010; `gfx::init` wraps the bootloader linear framebuffer; `[ITISYOU:INFO] gfx width=1280 height=720 bpp=3 bgr=true`, stages B170/B180 emitted; `gfx_available`, `gfx_info_sane` |
| GFX-002 | Framebuffer drawing primitives correct | IMPLEMENTED+VERIFIED | `gfx_fill_pixel`, `gfx_draw_glyph` (pixel read-backs), `gfx_clip_out_of_range` (no OOB write), `gfx_hash_deterministic` (CRC region hash stable) |
| GFX-003 | Back buffer → framebuffer format conversion (BGR/bpp) | IMPLEMENTED+VERIFIED | single `present()`; on-screen pixels read back at the compositor's exact palette values in the desktop screendump |
| COMP-001 | Compositor renders a desktop produced by the OS | IMPLEMENTED+VERIFIED | `composite()` draws wallpaper + top bar + windows + cursor; `desktop.ppm` screendump shows the OS-rendered desktop (colours match the compositor palette exactly) |
| COMP-002 | A window is composited to the screen | IMPLEMENTED+VERIFIED | `comp_window_fill`, `comp_window_composited` (fill a window, read the pixel back off the composited framebuffer) |
| COMP-003 | Cross-process window isolation (ownership) | IMPLEMENTED+VERIFIED | `comp_cross_owner_rejected` — a non-owner draw returns `NotOwner`; windows removed on owner exit (`gui_window_released_on_exit`) |
| COMP-004 | Out-of-bounds / bad-size draws rejected | IMPLEMENTED+VERIFIED | `comp_fill_out_of_bounds_rejected`, `comp_bad_size_rejected` |
| GUI-001 | Ring 3 renders a window via validated syscalls (no direct FB) | IMPLEMENTED+VERIFIED | `gui_create/fill/text/present` copy request structs from validated user memory; `user/gui-demo` prints `RING3-GUI-OK`; `gui_ring3_app_exit` |
| GUI-002 | A Ring 3 process's rendering appears on screen | IMPLEMENTED+VERIFIED | `gui_ring3_app_rendered` — the kernel reads the userspace window's 0xFF8800 pixel back off the composited screen |
| INP-001 | Real PS/2 keyboard input via IRQ1 | IMPLEMENTED+VERIFIED | `desktop-input-bios`: monitor `sendkey h e l l o` → `[ITISYOU:INPUT] key=h..o`; host-tested scancode decoder (`input_decoders`) |
| INP-002 | Real PS/2 mouse input via IRQ12 | IMPLEMENTED+VERIFIED | `desktop-input-bios`: monitor `mouse_move`/`mouse_button` → `[ITISYOU:INPUT] mouse dx=-20 dy=-15`, button `l=1`→`l=0`; cursor moves on screen; host-tested mouse decoder (incl. malformed packets) |
| INP-003 | Input IRQs do not deadlock the compositor/timer | IMPLEMENTED+VERIFIED | `set_input_irqs_enabled` masks interrupts around the PIC write; mouse IRQ accumulates deltas lock-free; full selftest + desktop run green with IRQs live |
| DESK-001 | Interactive desktop reacts to keyboard + mouse | IMPLEMENTED+VERIFIED | `desktop` command → `[ITISYOU:MODE] desktop`, `DESKTOP-READY`, live window updates, `DESKTOP-INPUT-VERIFIED keys=5 mouse=4` |
| DESK-002 | Graphics/input proven with automated verification (no faked UI) | IMPLEMENTED+VERIFIED | selftest 15 graphics tests + `desktop-input-bios` monitor-injection test + OS-produced `desktop.ppm` screendump; no host rendering anywhere |
| REG-V05 | V0.1–V0.4 regressions remain green under V0.5 | IMPLEMENTED+VERIFIED | full selftest **pass=78 fail=0** (BIOS) incl. all boot/userspace/preemption/storage tests; full local matrix Success |
| CI-V05 | CI green with V0.5 QEMU coverage | IMPLEMENTED+VERIFIED | run 33628363046 success on ubuntu-24.04 (fmt, split clippy, host tests, images, boot BIOS/UEFI, selftest BIOS+UEFI with graphics asserts + B170/B180, shell 0.5.0-dev, desktop-input monitor-injection leg, panic, two-boot fs-persist, website, gitleaks); tag v0.5.0 on commit 5be3c0c |
| WEB-V05 | os.itisyou.app reflects V0.5 truthfully | IMPLEMENTED+VERIFIED | live at v0.5.0-dev commit 71dbc5a, milestone "V0.5 — Graphics + Input + Basic Desktop/Compositor", verified modules (graphics/compositor/gui-syscalls/input-keyboard/input-mouse/desktop), OS-rendered desktop screendump published on /build, roadmap corrected (V0.1–V0.4 Verified, V0.5 current), browser+HTTP verified, zero console errors |

## V0.6 — Hardware Expansion

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| DEV-001 | Generic device/driver model (probe/bind, device table) | IMPLEMENTED+VERIFIED | ADR-0011; `device::{Device,Driver,enumerate,bind_drivers}`; B190; `devmodel devices=7`; `device_model_enumerated` |
| DEV-002 | Deterministic device enumeration + class decode | IMPLEMENTED+VERIFIED | fixed DRIVERS registry, ordered PCI scan; `device_class_decoded`; 7 QEMU devices (host/ISA bridge, IDE, VGA, e1000, NVMe) reported |
| PCI-BAR | BAR sizing (32/64-bit), non-destructive probe | IMPLEMENTED+VERIFIED | `probe_bars` (write-ones/read-mask/restore, decode disabled); host `computes_bar_sizes`; `device_nvme_bar_sized`, `device_bar_probe_nondestructive` |
| PCI-CAP | Capability-list walking (MSI/MSI-X/PCIe/PM) | IMPLEMENTED+VERIFIED | `walk_capabilities` (bounded, loop-guarded); NVMe reported `caps=[MSI-X,PCIe,PM]`; `device_caps_walked`; host `walks_normal_cap_chain` |
| PCI-ADV | Adversarial malformed/circular config handling | IMPLEMENTED+VERIFIED | host `cap_walk_terminates_on_circular_list`, `_self_loop`, `_rejects_out_of_range_pointer`; USB `hid_walk_tolerates_truncation` |
| USB-001 | USB host controller init + root-port reset | IMPLEMENTED+VERIFIED | UHCI driver; `uhci port_reset port=0x10`; `uhci ready` |
| USB-002 | Device enumeration over control transfers | IMPLEMENTED+VERIFIED | GET_DESCRIPTOR(device) → real QEMU usb-kbd `usb device vendor=0x0627 product=0x0001`; SET_ADDRESS/CONFIG/PROTOCOL; host descriptor-parse tests |
| USB-003 | Descriptor parsing + HID endpoint discovery | IMPLEMENTED+VERIFIED | `find_hid_interrupt_in` (host-tested) → `usb hid iface=0 proto=1 ep=1 kind=keyboard` |
| USB-HID | Real USB HID keyboard input (first USB class) | IMPLEMENTED+VERIFIED | interrupt-IN transfer; injected `sendkey g` → `[ITISYOU:INPUT] key=g src=usb`; host HID-decode tests |
| USB-XHCI | xHCI controller | NOT APPLICABLE (deferred) | UHCI chosen as the smallest QEMU-complete controller for *verified* HID input; xHCI is the documented V0.8 follow-up (ADR-0011) |
| INP-UNI | Unify PS/2 + USB HID behind one event stream | IMPLEMENTED+VERIFIED | `input::{feed_usb_*,pump_usb}` feed the same `InputEvent` queue; desktop consumes both; `key=g src=usb` → `DESKTOP-INPUT-VERIFIED` |
| AUD-001 | Audio controller init (codec reset/unmute) | IMPLEMENTED+VERIFIED | AC97 driver; `ac97 init codec_ready=true`; `driver_bound name="ac97"` |
| AUD-002 | Generated PCM samples traverse the output path | IMPLEMENTED+VERIFIED | `ac97 play bufs=8 civ=7 halted=true` (all buffers DMA-consumed) + captured WAV asserted non-silent by the runner — not init-only |
| AUD-CAP | Audio input/capture | NOT APPLICABLE (deferred) | output-only PCM path for V0.6; capture deferred (KNOWN_LIMITATIONS) |
| HWD-001 | Hardware discovery via kernel + userspace tool | IMPLEMENTED+VERIFIED | `lsdev` shell command; Ring 3 `/bin/lsdev` via `devinfo` → `RING3-LSDEV-OK count=7`; `device_userspace_lsdev` |
| USR-DEV | Userspace device access without hardware authority | IMPLEMENTED+VERIFIED | `SYS_DEVINFO` copies a record into a validated user buffer; all port I/O/config stays in-kernel; userspace lsdev uses no direct hardware |
| IRQ-MOD | IRQ modernization (APIC/MSI) | NOT APPLICABLE (deferred) | all drivers polled → verified PIC timer/PS-2 path untouched; MSI/MSI-X capabilities detected + reported; APIC deferred to V0.8 to avoid regressing a verified interrupt path (ADR-0011) |
| REG-V06 | V0.1–V0.5 regressions remain green under V0.6 | IMPLEMENTED+VERIFIED | full selftest **pass=84 fail=0** (BIOS) incl. all boot/userspace/preemption/storage/graphics/device tests; full local matrix Success |
| CI-V06 | CI green with V0.6 QEMU coverage | IMPLEMENTED+VERIFIED | run 33635955858 success on ubuntu-24.04 (fmt, split clippy, host tests, images, boot BIOS/UEFI, selftest BIOS+UEFI, shell 0.6.0-dev, desktop-input, **AC97 audio + WAV**, **USB UHCI + HID**, panic, fs-persist, website, gitleaks); tag v0.6.0 on commit cccf5a7 |
| WEB-V06 | os.itisyou.app reflects V0.6 truthfully | IMPLEMENTED+VERIFIED | live at v0.6.0-dev commit 14f3971, milestone "V0.6 — Hardware Expansion", verified modules (device-model/pci-caps/usb-uhci/usb-hid/input-unified/audio-ac97/devinfo-syscall), "Detected hardware" evidence section on /build (7-device enumeration + USB/audio proof), roadmap corrected (V0.6 current), browser+HTTP verified, zero console errors |

## V0.7 — System Platform

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| CAP-001 | Explicit capability model, default deny | IMPLEMENTED+VERIFIED | ADR-0012; syscall dispatch gates spawn/wait/ipc/gui/dev/fs_read; `cap_default_deny` — sandbox-probe with 0 caps has all 7 privileged classes refused with ERR_PERM (`SANDBOX-DENIED-OK`) |
| CAP-002 | Least privilege / caps enforced at kernel boundary | IMPLEMENTED+VERIFIED | per-process caps published per quantum; checks at the single dispatch boundary; services run with exact declared caps (`caps=0x2` markers) |
| CAP-003 | Delegation only through controlled paths, never amplifying | IMPLEMENTED+VERIFIED | `spawn` inherits exactly; `spawn_caps` = parent ∩ requested; `cap_delegation_no_amplify` — child requesting gui from a gui-less parent finds gui denied, delegated fs_read works (`CAP-DELEGATION-OK`); host `delegation_never_amplifies` |
| CAP-004 | Denied access is actually denied (adversarial) | IMPLEMENTED+VERIFIED | sandbox-probe exits 0 ONLY if every escape attempt returned ERR_PERM; `cap_denials_audited` (≥7 audit denial records) |
| SBX-001 | FS visibility restricted by sandbox prefixes | IMPLEMENTED+VERIFIED | `fs_read` checks normalized path vs prefixes (`is_within`, component-wise); `fs_sandbox_enforced` + `FS-SANDBOX-OK` (out-of-prefix, foreign app dir, traversal `/etc/../bin/init`, root-escape all ERR_PERM; in-sandbox miss stays ERR_NOENT) |
| SBX-002 | Apps cannot interfere with other apps/services/kernel | IMPLEMENTED+VERIFIED | per-process address spaces + GUI ownership (V0.3/V0.5) + caps + fs sandbox; hostile programs (crashd fault, sandbox-probe escapes) contained with kernel + peers unaffected; `svc_no_leaked_processes` |
| SVC-001 | Service model with supervisor, states, diagnostics | IMPLEMENTED+VERIFIED | services.rs registry (binary+deps+exact caps); states Stopped/Running/Done/Restarting/Failed; `[ITISYOU:SVC]` marker per transition; `svc` command shows real state |
| SVC-002 | Deterministic startup ordering + dependency handling | IMPLEMENTED+VERIFIED | Kahn order in host-tested kernel-core (`orders_respecting_dependencies`); `start ... order=0/1/2` markers (echod before its dependents) |
| SVC-003 | Dependency cycles rejected | IMPLEMENTED+VERIFIED | host `detects_cycles` (direct/self/indirect) + in-kernel `svc_cycle_detected` |
| SVC-004 | Crash containment + bounded restart policy | IMPLEMENTED+VERIFIED | crashd #PF contained, restarted exactly 3×, then Failed (`svc_crash_restart_bounded`, `state=failed ... restarts=3`); no restart storm |
| SVC-005 | A service actually serves (not just runs) | IMPLEMENTED+VERIFIED | echod served 3 IPC pings from the dependent client (`ECHOD-SERVED-3`, `SVC-CLIENT-OK`) |
| APP-001 | App identity/manifest with requested capabilities | IMPLEMENTED+VERIFIED | strict manifest (name/version/caps; unknown keys/caps + duplicates = errors); 8 host tests incl. hostile names/versions |
| APP-002 | Launch through the platform, manifest-only caps | IMPLEMENTED+VERIFIED | `pkg_launch_manifest_caps` — hello-app's granted fs_read works AND its out-of-manifest gui attempt is denied (`HELLO-APP-OK`) |
| PKG-001 | Smallest secure package format, integrity-verified | IMPLEMENTED+VERIFIED | ITPKG (header+manifest+ELF+SHA-256); strict parse (exact lengths); FIPS-vector-tested SHA-256; verified at install AND re-verified at launch |
| PKG-002 | Malformed/corrupt packages refused, store untouched | IMPLEMENTED+VERIFIED | `pkg_corrupt_rejected` (bit-flipped payload → DigestMismatch), `pkg_evil_manifest_rejected` (digest-valid pkg demanding undefined capability), `pkg_store_unchanged_after_refusals`; host adversarial length/truncation/trailing tests |
| PKG-003 | Deterministic, auditable install/uninstall ops | IMPLEMENTED+VERIFIED | version-numbered store; every op emits `[ITISYOU:PKG]` + audit records; ITFS `remove` (atomic superblock commit) |
| UPD-001 | Staged/atomic update | IMPLEMENTED+VERIFIED | stage `.pkg` → commit `.ok` = one crash-atomic superblock transition; `pkg_update_atomic` (v2 active, v1 rollback target) |
| UPD-002 | Rollback restores previous version | IMPLEMENTED+VERIFIED | `pkg_rollback_atomic` (remove newest `.ok`; v1 active again; demoted pkg kept as evidence) + relaunch green |
| UPD-003 | Interrupted update never activates; recovery detects it | IMPLEMENTED+VERIFIED | `pkg_interrupted_never_activates` + two-boot QEMU legs `update-interrupt`/`update-recovery`: staged v2 survives reboot as orphan, recovery removes it (`[ITISYOU:RECOVERY]` + audit), v1 launches |
| UPD-004 | Cryptographic signatures/authenticity | NOT APPLICABLE (deferred) | integrity = SHA-256 (verified); signatures need key provisioning + root of trust the platform lacks; architecture documented in ADR-0012 |
| UPD-005 | Kernel/system-image update | NOT APPLICABLE (deferred) | the OS does not own its boot media in the QEMU harness; app-store staged/commit/rollback is the designed mechanism for it (ADR-0012, V0.8+) |
| REC-001 | Known-good state + recovery path, forensic evidence kept | IMPLEMENTED+VERIFIED | ITFS double-buffered superblocks (V0.4) + recovery scan (reports + audits BEFORE cleanup); demoted/orphaned packages retained until an explicit recovery pass |
| AUD-001 | Privileged actions + denials recorded (actor/action/cap/result/seq) | IMPLEMENTED+VERIFIED | audit ring + `[ITISYOU:AUDIT] seq/tick/pid/action/cap/result` markers; `pkg_audit_trail`, `cap_denials_audited`; no payload contents logged |
| AUD-002 | AI-ready provenance path (intelligence ≠ authority) | IMPLEMENTED+VERIFIED | request → capability check → deterministic service → action → audit is the ONLY privileged path; no AI in kernel; documented in ADR-0012/SECURITY_MODEL |
| UI-001 | Real platform state exposed to the user | IMPLEMENTED+VERIFIED | `svc` (service states), `pkg list` (versions/active/staged), `audit` (trail), `run … [caps] [prefix]` — all read live kernel/platform data |
| FSL-001 | Platform filesystem layout | IMPLEMENTED+VERIFIED | initramfs: `/bin` (system), `/pkgs` (install media), `/etc` (config, generated version); persistent ITFS = package store + writable state; app view = `/apps/<name>` + `/etc` |
| REG-V07 | V0.1–V0.6 regressions green under V0.7 | IMPLEMENTED+VERIFIED | selftest **pass=106 fail=0** (all prior suites incl. graphics/USB/audio/devices); legacy-full caps keep pre-platform launches unchanged |
| CI-V07 | CI green with V0.7 QEMU coverage | IMPLEMENTED+VERIFIED | GitHub Actions run [33810268090](https://github.com/leelaravind/itisyou-os/actions/runs/33810268090) on `1f08bf5` completed successfully on ubuntu-24.04; full CI gate passed including V0.7 `platform-bios`, `update-interrupt`, and `update-recovery` legs, host tests, website build, and secret scan |
| WEB-V07 | os.itisyou.app reflects V0.7 truthfully | IMPLEMENTED+VERIFIED | staging `https://os-itisyou-app-staging.kpleelaaravind.workers.dev` verified first (Worker version `a816d6cd-0d6b-48a8-a87c-ac3c1fa1c353`), then production `https://os.itisyou.app` (Worker version `bf677d19-6f34-4492-a37a-5c33e5a7200d`); HTTP/TLS, V0.7 platform content, status metadata, security headers, responsive CSS, 404 handling, and browser journey were verified with zero console errors |

## V0.8 - Secure Platform + Networking Foundation

V0.8 evidence is recorded incrementally. IMPLEMENTED+VERIFIED requires a
passing host/QEMU test and a machine-readable artifact; design work alone is
not verification. Each IMPLEMENTED+VERIFIED row names the artifact that proves
it. `v0.8.0` was tagged with NET08-003 and NET08-004 in a state ("NOT DONE")
outside the plan's vocabulary; the 2026-09-13 audit restated them as
`NOT APPLICABLE` with the explicit roadmap amendment that moved them to V0.9,
where they are requirements NET09-001..003 — deferred and tracked, not
dropped (see the V0.8.1 section below).

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| HARN08-001 | QEMU harness is deterministic; boot-path regressions fail fast and self-diagnose | IMPLEMENTED+VERIFIED | Root cause was guest-side, not a harness race: an unstripped 26.8 MiB kernel ELF (17 MiB of DWARF in 19 embedded user programs) took 53 s to load through the bootloader's real-mode INT 13h path, past every leg deadline. Stripping in `kernel/build.rs` + `image-builder` -> first-output 53 s -> 6.8 s; same bug class in `uhci.rs` (spin-count "timeouts") fixed with `interrupts::Deadline` on the PIT. Harness now attributes failures per phase (`LaunchFailure`/`SerialConnectFailure`/`NoSerialOutput`/`Stalled`/`Timeout`), fails fast, records `connect_ms`/`first_output_ms`, and always writes its verdict into the serial log. Evidence: `cargo test -p qemu-runner` 6/6 (`tools/qemu-runner/tests/harness_startup.rs` driving `stub-qemu`); full matrix 14/14 Success on two consecutive end-to-end runs; matrix wall-clock ~15 min -> ~2 min; commit d1f2f9c |
| CAPH-001 | Opaque generation-checked resource-scoped handles | IMPLEMENTED+VERIFIED | `kernel_core::capability::CapabilityTable`; `cargo test -p kernel-core` 126 passed; QEMU `cap-handle-uefi` Success (29,363 ms) proves Ring 3 receives opaque handles and a forged generation is denied (`CAPH-FORGED-DENIED`) in `artifacts/qemu/cap-handle-uefi.result.json` |
| CAPH-002 | Owner binding, bounded delegation, revocation, expiry | IMPLEMENTED+VERIFIED | handles are the enforcement path, not a parallel record: `cap-handle-probe` drives the real syscalls and each of a forged generation, an expired handle and a revoked handle is refused (`CAPH-FORGED-DENIED`/`CAPH-EXPIRED-DENIED`/`CAPH-REVOKED-DENIED`/`CAPH-ENFORCEMENT-OK`) with the precise reason in the audit trail (`reason=expired`, `reason=no_handle`); QEMU `platform-bios` Success (31,681 ms), `artifacts/qemu/platform-bios.result.json` |
| SVC08-001 | Long-running Ring 3 services co-scheduled with the shell | IMPLEMENTED+VERIFIED | `tickd` (IPC-only) and `flapd` (zero caps) start at boot and are supervised for the life of the system; the shell's idle path pumps them (`services::pump`) instead of spinning, `bg` co-schedules a client with them, and `SYS_UPTIME` lets a daemon pace itself in real ticks. Evidence in `artifacts/qemu/services-bg-bios.result.json` (Success, 31,838 ms): heartbeats stamped with real kernel ticks at a fixed 2 s cadence; two `bg` clients complete IPC round trips against the same instance (`TICKD-SERVED n=2`, which a restarted service could not print); a daemon's clean exit is treated as a fault and restarted under the bounded policy, then `bg_failed name=flapd restarts=3`; the on-demand supervisor still reports `svc: started=3 done=2 failed=1 restarts=3` alongside them and the shell still accepts commands afterwards |
| SVC08-002 | Background services hold only their declared authority | IMPLEMENTED+VERIFIED | `bg_start name=tickd pid=1 caps=0x2 long_running=true` / `bg_start name=flapd pid=2 caps=0x0 long_running=true` — persistence grants a service nothing; there is still no privileged daemon (ADR-0014) |
| FS08-001 | Capability-scoped userspace filesystem writes | IMPLEMENTED+VERIFIED | `SYS_FS_WRITE/DELETE/LIST` behind the Filesystem WRITE right AND the process sandbox — two independent checks. Overwrite is ONE crash-atomic superblock commit (`SuperBlock::replace`), so an interrupted overwrite leaves the old contents rather than a truncated file. Evidence `artifacts/qemu/fs-write-bios.result.json`: create, overwrite to a different length, list, delete, four refusals each for a different reason, a writer confined to `/pkgs` refused by the sandbox while holding `fs_write`, and `fs-write-denied` (holding `fs_read` only) refused every mutation while still reading and listing the same store. `fs-write-persist`: a fresh guest on the same disk reads back what Ring 3 wrote |
| PKG08-001 | Signed package manifests and trusted-key verification | IMPLEMENTED+VERIFIED | Ed25519 in `kernel_core::ed25519`, validated against RFC 8032 §7.1 vectors (public keys, signatures, verification) — 24 host tests; every curve constant derived rather than transcribed. ITPKG002 signs a context string, the declared lengths and the content digest. Integrity and authenticity are separate steps, so `platform-bios` shows a trusted package install and launch (`signature result=ok signer=…`) and three INTACT packages refused for three distinct reasons: `unsigned`, `untrusted_signer`, `bad_signature`, with `DigestMismatch` still refused earlier on integrity. Trust root compiled in and derived at build time from the signing seed (ADR-0016) |
| NET08-002 | Network capability/policy enforcement | IMPLEMENTED+VERIFIED | `udp_bind` is checked against the Network handle SCOPED TO THE PORT; every later datagram call re-checks the socket's own port, so a handle narrowed or revoked after the bind stops working at the next use. `net-bios`: `net-probe` (holding `network`) completes a UDP round trip and a DNS lookup; `net-denied` (no capabilities) is refused every network syscall with `action=… cap=0x0 result=denied detail="kind=network reason=no_handle"` |
| IRQ08-001 | APIC/IOAPIC interrupt routing | IMPLEMENTED+VERIFIED (scope stated) | Local APIC enabled and PROVED to deliver: a one-shot APIC timer interrupt on vector 0x41 (`apic_timer delivered=true count=1`). I/O APIC discovered, mapped, and a redirection entry programmed and read back (`readback=ok`) but left MASKED — line IRQs stay on the verified PIC path, and the evidence says `masked=true` so it cannot be misread as line-based delivery (ADR-0017). `artifacts/qemu/irq-bios.result.json` |
| IRQ08-002 | MSI/MSI-X interrupt delivery | IMPLEMENTED+VERIFIED | MSI-X programmed on the NVMe controller and delivered by a REAL block read's completion: `msix_enabled dev=00:04.0 vector=0x42`, `msix armed=true block_read=true delivered=true`, `spurious=0`. QEMU's e1000 exposes no MSI capability at all, so the boot report names which devices are MSI-X capable rather than claiming the NIC is |
| USB08-001 | xHCI enumeration and HID path | IMPLEMENTED+VERIFIED | A second, structurally different host controller sharing only `kernel_core::usb` with UHCI. `xhci-hid-bios`: controller reset, command/event rings, Enable Slot, Address Device, two control IN transfers (`xhci device vendor=0x0627`), HID endpoint located (`iface=0 endpoint=0x81`), Configure Endpoint plus SET_CONFIGURATION and SET_PROTOCOL on the device, and a real injected keypress over an interrupt transfer: `XHCI-HID-REPORT bytes=8 … ascii=g`, `[ITISYOU:INPUT] key=g src=xhci` (ADR-0018) |
| AUD08-001 | Persistent tamper-aware audit records | IMPLEMENTED+VERIFIED | Records hash-chained (`kernel_core::audit_chain`, 10 host tests covering alter/delete/reorder/insert/truncate); the chain is extended BEFORE the bounded ring drops anything, so a busy system cannot lose evidence by being busy, and it continues across boots from the head it recovered. Three-boot evidence: `audit-persist-write` (saved), `audit-persist-verify` (a fresh guest recomputes the chain and gets the same head, `chain boot=1`), `audit-persist-tamper` (`trail_recovered status=TAMPERED`). Boot-time recovery mounts READ-ONLY so booting can never reformat a disk |
| HARD08-001 | SMEP/SMAP, guard pages, pointer/W^X hardening | IMPLEMENTED+VERIFIED | SMEP, SMAP and UMIP enabled from CPUID and reported from CR4. SMAP inverts the default: the kernel is forbidden to touch user pages except in the three declared windows, each bracketed by a `UserAccess` guard so an early return cannot leave the window open. The runner requests `+smep,+smap,+umip`, so ALL 24 legs run with supervisor-mode protection on. `harden-bios`: `sgdt` from Ring 3 is a contained #GP with `HARDEN-LEAK-SGDT` forbidden; the stack probe writes exactly 16 pages then faults at `0x7ffffdf000` with `STACKGUARD-LEAK` forbidden; a W^X segment is refused at load; a Ring 3 read of a kernel address still faults |
| NET08-000 | Host-tested Ethernet/IPv4 protocol layer (parse, build, checksum) | IMPLEMENTED+VERIFIED | `kernel_core::net::{checksum,eth,ipv4}` — allocation-free, borrows from the caller's DMA buffer, refuses what it does not fully understand (VLAN tags, fragments, bad IHL/TTL/checksum, every truncation boundary) rather than guessing; 43 of the 174 `cargo test -p kernel-core` cases. This is protocol logic only — it moves no packets |
| NET08-001 | QEMU NIC, Ethernet/ARP/IPv4/ICMP/UDP data path | IMPLEMENTED+VERIFIED | e1000 driver with polled RX/TX descriptor rings, an ARP cache, ICMP echo client AND responder, a bounded UDP socket table, and a DNS resolver. Verified against the runner's OWN host-side Ethernet peer (`tools/qemu-runner/src/wire.rs`) over a `dgram` netdev — an independent byte-level implementation, so a bug in the guest's codec cannot cancel itself out against the same code. `net-bios` proves both directions (the guest answering the peer's ARP request and ping, not only initiating), zero bad checksums as computed by the independent peer, and that five hostile frames are refused (`rx_malformed=3 rx_unwanted=2`) and answered by NOTHING (`replies_to_hostile=0`) |
| NET08-003 | TCP | NOT APPLICABLE (deferred by explicit roadmap amendment → NET09-001) | Not delivered in V0.8: the stack is datagram-only. A correct TCP needs a retransmission timer, window management and a connection state machine whose failure modes are exactly the ones a half-implementation hides, so it was moved to V0.9 in `docs/ROADMAP.md` ("V0.9 — Transport, …") rather than shipped half-built. Carried forward as NET09-001; originally recorded as "NOT DONE", a state outside the vocabulary |
| NET08-004 | DHCP, IPv6, routing beyond one gateway | NOT APPLICABLE (deferred by explicit roadmap amendment → NET09-002/003) | Not delivered in V0.8: the address plan is static with one gateway, no IPv6, no fragmentation, no ICMP error generation (an unreachable port is dropped silently, which is also what makes "answers nothing hostile" checkable). DHCP and IPv6 foundations moved to V0.9 in `docs/ROADMAP.md`; carried forward as NET09-002 (DHCP) and NET09-003 (IPv6). Originally recorded as "NOT DONE" |
| REG-V08 | V0.1-V0.7 regression matrix remains green | IMPLEMENTED+VERIFIED | every V0.1-V0.7 leg passes alongside the V0.8 subsystems, and now with SMEP/SMAP/UMIP enabled on every leg: 24/24 QEMU legs Success, selftest pass=113 fail=0 |
| CI-V08 | Full V0.8 CI/QEMU matrix green | IMPLEMENTED+VERIFIED | CI run `33987853808` (commit `e6e3496`) success on ubuntu-24.04: format, split clippy, host tests, image build, and all 24 QEMU legs — including the V0.8 networking, xHCI, interrupt-modernization, filesystem-write, persistent-audit and hardening legs. Earlier intermediate evidence: CI run `33955559882` (commit `6be344e`) success on ubuntu-24.04 with the then-15 QEMU legs. The release stamp commit `84d6b24` (tag `v0.8.0`) is green in CI run `33988391309` |
| WEB-V08 | Production website reflects only verified V0.8 behavior | IMPLEMENTED+VERIFIED | `status/current.json` stamped to the CI-verified commit `e6e3496`; staging (`edaab8e1-37e3-4c3a-a13b-398494ab988c`) verified first, then production (`a3544b0a-5fe1-44ab-8dfc-b2bd4a3dda0d`). Live checks on https://os.itisyou.app: 16/16 routes 200, `/nope` → 404, strict CSP + `nosniff` + `DENY`, TLS 200, zero `<script>` tags, and the page rendering `v0.8.0 · Current Build VERIFIED · COMMIT e6e3496d129f5f9d753ddd90b875bccafbe5cefe` alongside every V0.8 module. The gaps are on the site too: TCP, DHCP and IPv6 appear as planned, not delivered |

## V0.8.1 — Release-integrity closeout (Phase 1 audit, 2026-09-13)

An independent audit of the repository, tags, CI, runtime evidence and the live
site (evidence priority: runtime → code → Git/CI → documentation) found that
the V0.8 *engineering* was real and still reproduces, but the V0.8 *release*
disagreed with itself: the `v0.8.0` kernel reported `itisyou-os 0.7.0-dev`,
two requirement rows carried a state outside the plan's vocabulary, and the
public site plus several documents still made V0.1-era claims ("no network
stack exists", "current milestone is V0.1"). A version whose artifact misnames
itself is a release defect, and the `v0.8.0` tag is immutable, so the
correction is a patch release rather than a rewritten tag. No kernel behaviour
changes in V0.8.1 beyond the version it reports.

Also recorded, not acted on: draft PR #1 (`W0-01: Freeze normative inputs`,
branch `w0-01-freeze-normative-inputs`, opened 2026-09-06) proposes a separate
architecture chain whose source documents are not in the repository; it is
unmerged, owner-gated, and outside `main`'s governance, so it was left
untouched.

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| AUD081-001 | The v0.8.0 engineering claims are re-verified from runtime evidence, not trusted from documents | IMPLEMENTED+VERIFIED | 2026-09-13: `scripts/verify.ps1` on `84d6b24` (plus the scratch-drive fallback below) → `VERIFY: OK` — doctor, fmt, both clippy gates, 292 host tests (kernel-core 278), **24/24 QEMU legs Success**, selftest pass=113 fail=0 on BIOS and UEFI, website 0 errors, secret scan clean over 275 files. Git: `v0.8.0` → `84d6b24` = `origin/main`, clean tree; CI `33988391309` green on that commit; https://os.itisyou.app serving `v0.8.0` |
| REL081-001 | Kernel, Cargo metadata, status metadata and tag name the same version | IMPLEMENTED+VERIFIED | Root cause: the workspace version was never bumped after V0.7 (`0.7.0-dev`), and the shell-test legs asserted that same stale string, so the drift was *tested in*. Fix: workspace `0.8.1`, both shell-test legs (local + CI) assert `itisyou-os 0.8.1`, status `0.8.1`. Evidence: local `verify.ps1` → `VERIFY: OK` with `shell-test-bios` requiring `itisyou-os 0.8.1`; CI run `34749240177` (commit `397ab80`) green including the same leg; the downloaded release images print `itisyou-os 0.8.1` on BIOS and UEFI |
| REL081-002 | Release drift fails the build instead of shipping | IMPLEMENTED+VERIFIED | `website/scripts/check-consistency.mjs` (in `npm run verify`, so every CI run): status version must equal the Cargo version; every requirement state must be in the vocabulary; a verified status may not carry unstarted rows; `--release` refuses any non-terminal row before tagging. Regression evidence: run against the `v0.8.0` tree it exits 1 naming exactly the three defects (version `0.8.0` vs `0.7.0-dev`, NET08-003, NET08-004); on the corrected tree it passes in the local gate and in CI run `34749240177` (website job) |
| REL081-003 | Requirement matrix uses only the plan §2 states | IMPLEMENTED+VERIFIED | NET08-003/004 restated as `NOT APPLICABLE` with the roadmap amendment that moved them to V0.9 (NET09-001..003); title no longer says "V0.1"; enforced by REL081-002 on every build |
| DOC081-001 | No document states a stale current-state claim | IMPLEMENTED+VERIFIED | Corrected: README ("current milestone is V0.1"), KNOWN_LIMITATIONS (V0.7-era bullets contradicting V0.8, stale "V0.8 work in progress" section), SESSION_CHECKPOINT ("V0.8 unreleased"), ROADMAP ("V0.7 current"), ADR-0013 status ("integration pending"), UNSAFE_INVENTORY (no rows for the V0.8 APIC/e1000/xHCI/SMAP code), BUILD_AND_RUN + website README (hard-coded `G:`), initramfs `welcome.txt` ("nothing here persists"), THREAT_MODEL/SECURITY_MODEL "V0.1" framing, RECOVERY/BUILD_AND_RUN toolchain name. Found by a full read of `docs/`, `README.md` and the site sources against the audited facts; UNSAFE_INVENTORY rows 33–39 checked against the source (`unsafe impl Send` + single-mutex ownership in e1000/xHCI, SMAP gating) |
| ENV081-001 | Build scratch and TEMP never fall back to C: | IMPLEMENTED+VERIFIED | `G:` (external scratch) was not attached, and `scripts/env.ps1` hard-coded `G:\claude-tmp`, so every gate script failed before doing anything. `env.ps1` now uses `G:\claude-tmp` when present and `E:\claude-tmp` otherwise; `doctor.ps1` checks `TEMP` is off C: and treats G: as optional. Evidence: `DOCTOR: OK` with `[OK] TEMP off C: E:\claude-tmp	mp` and `[OK] scratch G: not attached; using E:\claude-tmp` in both 2026-09-13 gate runs |
| WEB081-001 | The public site makes no stale or false claim | IN PROGRESS | Live `v0.8.0` site said "no network stack exists in V0.1" (security), "V0.8+ is documented research direction" (roadmap) and V0.1-era release rules (releases) |
| WEB081-002 | Staging and production verified in a real browser, not only over HTTP | IN PROGRESS | V0.8 was verified over HTTP only (the browser extension was unavailable and it was recorded as such). New `website/scripts/browser-verify.mjs` drives headless Chromium over the DevTools protocol: every route at 1440 px and 390 px, console errors, exceptions, CSP/blocked-resource log errors, failed requests, horizontal overflow, landmarks, broken images, every internal link, the 404 page and the security headers |
| REL081-004 | Release boot images are bit-reproducible | IN PROGRESS | Found: BIOS images were identical across rebuilds but UEFI images were not — the `gpt` crate stamps random disk/partition GUIDs into every image. Fix: `tools/image-builder/src/gpt_normalize.rs` derives the GUIDs from the image's own content (GUID/CRC fields zeroed, SHA-256) and recomputes every GPT CRC, then re-verifies both headers before the image is kept; 5 host tests (CRC-32 vector, two random-GUID images normalize identically, different content ⇒ different identity, non-GPT/truncated refused, corrupted CRC detected). Evidence: two local builds → identical manifests for all 8 images; normalized UEFI images pass boot-smoke-uefi, selftest-uefi (113/0) and the two-boot fs-persist UEFI legs. Cross-OS caveat, stated not hidden: a Windows build embeds Windows-style panic-location paths, so the *release* images are the CI-built (Linux) ones, and reproducibility is shown by independent CI runs producing identical digests |
| REL081-005 | A public, checksummed, boot-tested download | IN PROGRESS | CI uploads the exact boot images (`boot-images` artifact) and prints their digests; the release downloads those bytes, checks SHA-256, boots them in QEMU (BIOS + UEFI), and publishes them at `/downloads/v0.8.1/` with `SHA256SUMS.txt` via `website/scripts/downloads.mjs stage` (refuses any byte not matching `website/src/data/downloads.json`); `downloads.mjs verify` re-downloads from the deployed site and checks size, SHA-256 and attachment headers. New `/download` page: version, maturity, commit, builder run, sizes, SHA-256, verify commands, QEMU instructions, tested environments, physical-hardware warning, known limitations |
| CI-V081 | CI green for the V0.8.1 release commit | IMPLEMENTED+VERIFIED | CI run [34749240177](https://github.com/leelaravind/itisyou-os/actions/runs/34749240177) on `397ab80` green on ubuntu-24.04: fmt, both clippy gates, host tests, image build + release digests + `boot-images` upload, all 24 QEMU legs, website (incl. the consistency gate), gitleaks |
| REG-V081 | Every V0.1–V0.8 leg still green at 0.8.1 | IMPLEMENTED+VERIFIED | Local `verify.ps1` → `VERIFY: OK` (24/24 QEMU legs, selftest 113/0 BIOS+UEFI, 292 host tests → 297 with the 5 GPT tests) and CI run `34749240177` (24/24 legs). The four UEFI legs were re-run on the GPT-normalized images: boot-smoke-uefi, selftest-uefi, fs-persist-write/verify all Success |

## Testing & verification

| ID | Requirement | Status | Evidence |
|---|---|---|---|
| TEST-001 | One-command verification gate exists | IMPLEMENTED+VERIFIED | `scripts/verify.ps1` full run: doctor OK, fmt OK, clippy OK, 41 host tests, 6/6 QEMU legs Success, website 0 errors, secret scan clean -> `VERIFY: OK - all applicable gates passed`; generated `artifacts/qemu/*.result.json` records the V0.7 platform and two-boot update/recovery evidence |
| TEST-002 | QEMU timeout/failure classification works | IMPLEMENTED+VERIFIED | classifications observed operating correctly during real debugging: Timeout (interactive halt), Panic (UEFI TooManyRegions), MissingMarkers (FIFO stall), Success; negative leg panic-test-bios green |
| SEC-001 | Unsafe inventory exists | IMPLEMENTED+VERIFIED | `docs/UNSAFE_INVENTORY.md` — 32 documented unsafe contracts (incl. gfx framebuffer, i8042 input, PCI BAR probe, AC97 + UHCI DMA/port I/O) |
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
