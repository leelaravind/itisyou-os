# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~11:30 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0`, `v0.2.0`, `v0.3.0` (v0.4.0 tagged after CI confirms)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.4 Preemptive Multitasking + Persistent Storage — verified
(commit `b1af235`)

## V0.4 verified additions (selftest pass=63 fail=0, BIOS+UEFI)

- **Preemptive scheduling** (interrupts.rs naked timer ISR, full trap-frame
  UserContext, iretq resume, proc.rs run-loop): non-yielding processes
  preempted; registers + address spaces preserved; infinite spinner cannot
  monopolize; cooperative sched coexists; no leaks/lost processes.
- **NVMe write + flush** (device/nvme.rs); **BlockDevice** trait write/flush.
- **ITFS** persistent filesystem (kernel-core/itfs.rs format logic +
  kernel/fs_disk.rs block I/O): double-buffered CRC superblocks, strict
  validation, crash-consistency (torn-superblock recovery).
- **Reboot persistence** (hard acceptance target): itisyou-fs-persist run
  twice on one disposable disk (UEFI) → FS-PERSIST-WROTE then
  FS-PERSIST-VERIFIED on a fresh guest.

## Gates / how to resume

- `scripts/verify.ps1` = full gate. Storage FS tests need the runner
  `--nvme-persist <path>` flag. `scripts/test.ps1` runs the whole matrix
  incl. the two-boot persistence test (UEFI).
- Known issue: the `itisyou-fs-persist` BIOS image does not boot (bootloader
  BIOS-stage quirk; UEFI image boots). First V0.5 task: root-cause it.
- Next milestone **V0.5** recommended start: a **second block-device backend
  (virtio-blk or AHCI)** behind the existing `BlockDevice` trait to prove the
  abstraction across drivers, then richer ITFS file ops (update/delete/
  directories) + data-write journaling. Also pending: PS/2 keyboard, stack
  guard pages, capability-handle IPC, scheduler priorities/fairness.

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu;
TEMP → G:\claude-tmp\tmp; wrangler OAuth on this machine; never mix
kernel/user + host packages in one cargo invocation; user programs build
static no-pie; user pointer validation uses the active CR3
(translate_active); preemption resume is iretq (full GPR restore).

## Blockers

None.
