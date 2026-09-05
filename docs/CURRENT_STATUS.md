# Current status — V0.7 System Platform

**Timestamp:** 2026-09-03 (E: UTC+1, Europe/London)  
**Branch:** `main` · **Repository:** `E:\Project\itisyou-os`  
**Milestone:** `V0.7 — System Platform` (evidence-backed, no hidden privileged daemons)

- Kernel, userspace, graphics and hardware foundations from prior milestones remain green: `REG-V07` is satisfied with selftest pass **106** / fail **0** (local run + CI).
- New V0.7 platform properties are implemented and verified:
  - capability default-deny + explicit process capabilities,
  - FS sandbox path-prefix confinement,
  - service registry/supervision with deterministic dependency order,
  - bounded restart policy and failure diagnostics,
  - managed package/app launch via manifests,
  - atomic update + rollback + reboot-recovery,
  - privileged action + denial audit trail.
- Required adversarial checks are present for denial enforcement, delegation, hostile manifests,
  malformed packages, service failures, dependency cycles, and interrupted/invalid updates.
- `status/current.json` is the source of website truth. It is stamped to
  verified commit `1f08bf5` after CI run `33810268090` and staging/production
  website verification.

### Release evidence in-tree (last run)

- Local canonical verification: `scripts/verify.ps1` → all gates green (verify marker at end: `VERIFY: OK - all applicable gates passed`).
- QEMU artifact suite produced with zero failures for required V0.7 legs:
  `platform-bios`, `update-interrupt`, `update-recovery` in `artifacts/qemu/`.
- Remote CI run `33810268090` is green and includes the same V0.7 QEMU
  coverage on ubuntu-24.04.
- Staging was verified before production. Production is live at
  `https://os.itisyou.app` with the V0.7 status source and security headers.

### Known boundary notes

- No user-space filesystem-write syscall yet; only `fs_read` is sandboxed.
- capabilities are static bit-flags (no revocation yet).
- package authenticity is integrity-only (SHA-256), signatures deferred to V0.8.
- one update + recovery trail is retained (`itfs` atomic superblocks + recovery reports).

### V0.8 implementation status

- **Capability handles are the enforcement path** (verified): a forged
  generation, an expired handle and a revoked handle are each refused on the
  real syscall path, with the reason recorded in the audit trail
  (`platform-bios`).
- **Long-running Ring 3 services** (verified, ADR-0014): `tickd`/`flapd` start
  at boot and are supervised for the life of the system; the shell's idle path
  gives them CPU instead of spinning; `bg` co-schedules a client with them so
  an IPC round trip with a live service is possible; a daemon's clean exit
  counts as a fault and is restarted under the same bounded policy, then
  marked Failed at the ceiling; `SYS_UPTIME` lets a daemon pace itself in real
  ticks rather than in load-dependent scheduling passes (`services-bg-bios`).
  They hold exactly their declared capabilities — persistence buys no
  authority, and there is still no privileged daemon.
- **Network protocol layer** (host-tested only): `kernel_core::net::{checksum,
  eth,ipv4}` parses and builds Ethernet II and IPv4 strictly — 43 host tests
  covering VLAN tags, fragments, bad IHL/TTL/checksum and every truncation
  boundary. It moves no packets: there is no NIC driver and no data path yet,
  so **no networking claim is made**.
- Local matrix after this slice: selftest **113** / fail **0**; 181 host tests;
  15/15 QEMU legs Success (`TEST: OK`).
- Filesystem writes, signed packages, the NIC data path, interrupt
  modernization, xHCI, persistent audit and hardening are not yet verified.
- `status/current.json` — the machine-readable source of website truth — is
  unchanged and still stamped to the verified V0.7 release. The three repo
  docs the site renders (ARCHITECTURE, TESTING, KNOWN_LIMITATIONS) now
  describe the verified V0.8 slices and the explicit absence of networking.
  Nothing has been deployed.

### Next milestone (V0.8)

Networking (NIC + IPv4/UDP + firewall-like constraints), xHCI, APIC/IOAPIC/MSI,
userspace FS writes behind capability constraints, and service-package ecosystem hardening.
