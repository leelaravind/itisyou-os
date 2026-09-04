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

- Implemented and directly host-tested: bounded opaque capability handles with
  generation checks, owner binding, resource scopes, rights intersection,
  explicit revocation, expiry, and owner teardown revocation.
- Capability handles now have a kernel-owned process registry, publication,
  listing/check ABI, and teardown revocation hooks; kernel compilation/QEMU
  enforcement evidence is still pending.
- Userspace services, filesystem writes, signed
  packages, networking, interrupt modernization, xHCI, persistent audit and
  hardening are not yet verified. No website claim has been changed.

### Next milestone (V0.8)

Networking (NIC + IPv4/UDP + firewall-like constraints), xHCI, APIC/IOAPIC/MSI,
userspace FS writes behind capability constraints, and service-package ecosystem hardening.
