# Session checkpoint — resumable state

**Timestamp:** 2026-09-03 ~18:20 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0` … `v0.6.0` (v0.7.0 pending until final CI + production verification)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.7 System Platform — verified in local matrix (selftest pass=106 fail=0; platform-bios + two-boot update-recovery green). CI/WEB stamping pending final remote verification.

## V0.7 verified additions

- **Capabilities** (kernel-core/caps + syscall gating): default-deny bits,
  per-quantum publication, spawn inherits / spawn_caps intersects (no
  amplification). New syscalls: SYS_FS_READ=13 (sandboxed), SYS_SPAWN_CAPS=14.
- **FS sandbox**: per-process path prefixes checked on the normalized path
  (kernel-core path::is_within — traversal-proof).
- **Audit** (kernel/audit.rs): ring + [ITISYOU:AUDIT] markers for every
  privileged action + denial; `audit` command.
- **Services** (kernel/services.rs + kernel-core/service): registry with deps
  + exact caps; deterministic cycle-checked order; bounded restart (3);
  [ITISYOU:SVC] markers; `svc` command. proc gains reap() +
  run_until_pid_idle_bounded().
- **Packages/updates** (kernel/platform.rs + kernel-core/{manifest,pkg,
  sha256,update}; ITFS atomic remove): ITPKG format; store `<app>.<v>.pkg`
  + `.ok`; install/rollback/recover each ONE atomic superblock commit;
  launch re-verifies digest + grants manifest∩launcher caps under the app
  sandbox (/apps/<name> + /etc); `pkg` command. Fixtures packed by build.rs
  (incl. corrupted + hostile-manifest) under /pkgs.
- New user programs: sandbox-probe, cap-parent/child, fs-probe, echo-svc,
  crashy-svc, svc-client, hello-app.
- Harness: NVMe test disks 16 MiB (774 KB debug ELFs need the room).

## Gates / how to resume

- `scripts/test.ps1` = full matrix incl. platform-bios + update-interrupt/
  update-recovery (two boots, one persistent disk). `scripts/verify.ps1` =
  full gate.
- Shell: `run <path> [caps|-] [prefix]`, `svc`, `pkg …`, `audit`.
- Next **V0.8**: networking (e1000 already enumerated at 00:03.0; polled
  RX/TX + ARP/IPv4/UDP fits the polled-driver + host-tested-parser pattern).
  Platform follow-ups: capability handles/revocation, signed packages,
  userspace FS writes, long-running services/userspace init.

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu;
TEMP → G:\claude-tmp\tmp; never mix kernel + host packages in one cargo
invocation; user programs build static no-pie; kernel build.rs has
kernel-core as a build-dependency (ITPKG packing); CURRENT_CAPS/sandbox
published per quantum in run_quantum; PIC-lock and compositor-lock ISR rules
unchanged.

## Blockers

None.
