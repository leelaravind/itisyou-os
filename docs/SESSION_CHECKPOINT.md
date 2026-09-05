# Session checkpoint — resumable state

**Timestamp:** 2026-09-05 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0` … `v0.7.0` (V0.8 is unreleased; no V0.8 tag yet)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.8 in progress. The released V0.7 System Platform is immutable at `v0.7.0` and was locally and remotely verified (selftest pass=106 fail=0; platform-bios + two-boot update-recovery green; CI run 33810268090 green; staging and production website verified).

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
- Next in **V0.8**: the e1000 driver + RX/TX data path (the NIC is already
  enumerated at 00:03.0 and the parsers in `kernel_core::net` are ready and
  host-tested, so what is missing is polled RX/TX rings plus ARP/ICMP/UDP
  on top of them), then signed packages, userspace FS writes, and a
  userspace init.

## V0.8 work checkpoint

The immutable V0.7 baseline is `v0.7.0` at `00b5eea`. V0.8 slices landed so
far, each with QEMU or host evidence:

1. **Capability handles** — `kernel_core::capability::CapabilityTable` (forged/
   stale-handle rejection, ownership and scope checks, bounded delegation,
   revocation, expiry, owner-teardown revocation), and they are now the actual
   enforcement path: `cap-handle-probe` drives the real syscalls and forged/
   expired/revoked handles are each refused with the reason audited
   (`platform-bios` leg).
2. **Long-running Ring 3 services** (ADR-0014) — `tickd` (IPC-only) and
   `flapd` (zero caps) start at boot via `services::start_background()` and are
   supervised for the life of the system. The shell's idle path calls
   `services::pump(1)` instead of spinning; `bg <path> [caps]` co-schedules a
   client with them (bounded at 3000 ticks); `ServiceDef::long_running` makes a
   daemon's clean exit a fault; `SYS_UPTIME` (19) lets a daemon pace itself in
   real ticks. `run_supervised` scopes both its status reset and its report to
   `REGISTRY` so background rows survive and are not miscounted. Evidence:
   `services-bg-bios` leg.
3. **Network protocol layer** — `kernel_core::net::{checksum,eth,ipv4}`, 43 of
   the 174 kernel-core tests. Parsers only; no NIC driver, no data path, and
   no networking claim anywhere in the docs or the website.

Local gate at this checkpoint: selftest pass=113 fail=0, 181 host tests,
15/15 QEMU legs Success (`TEST: OK`). `status/current.json` is untouched and
still stamped to V0.7; nothing has been deployed.

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu;
TEMP → G:\claude-tmp\tmp; never mix kernel + host packages in one cargo
invocation; user programs build static no-pie; kernel build.rs has
kernel-core as a build-dependency (ITPKG packing); CURRENT_CAPS/sandbox
published per quantum in run_quantum; PIC-lock and compositor-lock ISR rules
unchanged.

## Blockers

None.

## Prior release evidence (V0.7)

Release evidence points to commit `1f08bf5`, CI run `33810268090`,
staging Worker `8ab2626a-88e2-4721-9956-632339d146c2`, and production Worker
`1c065f8f-00be-46d7-ada2-7eb2238a8ebb`; the release tag is applied after the
closing evidence commit and clean-tree check.
