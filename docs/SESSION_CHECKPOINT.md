# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~09:40 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0` (kernel foundation), `v0.2.0` (userspace), `v0.3.0` (this milestone)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.3 Process Isolation + Storage — verified (commit `d83942b`)

## V0.3 verified additions (evidence: selftest pass=50 fail=0, BIOS+UEFI)

- **Per-process page tables** (memory/aspace.rs): private L4, shared kernel
  mappings, safe CR3 switching, cross-process memory isolation proven,
  leak-free teardown (frame-exact accounting).
- **Concurrent processes** (proc.rs): resumable sysretq contexts,
  cooperative run-loop, spawn/wait/exit, deterministic interleaving.
- **Syscalls**: write/exit/yield/getpid/spawn/wait/msg_send/msg_recv with
  active-CR3 user-buffer validation.
- **IPC** (ipc.rs): bounded kernel message channels.
- **Storage**: PCI enumeration (device/pci.rs), block abstraction
  (device/block.rs), read-only NVMe driver (device/nvme.rs) reading LBA 0
  from a generated disposable disk. Boot stage B160.

## Gates / deployment

- `scripts/verify.ps1`: VERIFY OK (55 host tests, 6-leg QEMU matrix,
  website, secret scan). CI run 33609090873 **green**.
- os.itisyou.app LIVE at v0.3.0-dev, commit d83942b, verified in browser.

## How to resume (any fresh session)

1. Read `CLAUDE.md` → `AGENT_OPERATING_RULES.md` + `docs/IMPLEMENTATION_PLAN.md`.
2. Dot-source `scripts/env.ps1`; `scripts/doctor.ps1`; `scripts/verify.ps1`.
3. Storage tests need the runner `--nvme` flag (generates a disposable disk).
4. Next milestone: **V0.4** — recommended start: preemptive user scheduling
   (timer-ISR trap-frame save/restore), then NVMe writes + a persistent
   filesystem on the block layer, then AHCI/virtio-blk. Other leftovers:
   PS/2 keyboard, guard pages, fork/exec-with-args, capability-handle IPC.

## Environment keys (details in docs/BUILD_AND_RUN.md)

nightly-2026-08-01 pin (upstream bootloader#579); toolchains on E:; QEMU
11.1.0 at E:\tools\qemu; TEMP → G:\claude-tmp\tmp; wrangler OAuth on this
machine; never mix kernel/user + host packages in one cargo invocation;
user programs build static no-pie (loader accepts only ET_EXEC); user
pointer validation uses the active CR3 (translate_active).

## Blockers

None.
