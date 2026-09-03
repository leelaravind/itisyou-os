# ADR-0012: System platform — capabilities, services, packages, updates, audit

**Status:** Accepted · 2026-09-02 (V0.7)

## Context

Through V0.6 the OS had strong *mechanical* isolation (address spaces, syscall
validation, GUI/window ownership, mediated device access) but coarse
*authority*: any Ring 3 process could use every syscall. V0.7 turns the
foundation into a coherent platform: explicit capabilities, supervised system
services, a managed application/package model, safe updates with recovery, and
an audit trail — designed so a future AI agent is just another untrusted
requester (**intelligence ≠ authority**).

## Decision — capability model (default deny)

- Explicit capability bits per process (`kernel_core::caps`): `spawn`, `ipc`,
  `gui`, `dev`, `fs_read`, plus reserved `audio`/`sys_admin`. The basic
  runtime (write/exit/yield/getpid) needs none; **every other syscall requires
  its bit** or returns `ERR_PERM` and is audited.
- Enforcement lives at the single syscall dispatch boundary; the current
  process's caps are published per scheduling quantum.
- **Delegation only narrows**: `spawn` inherits the parent's set exactly;
  `spawn_caps` grants `parent ∩ requested`. A child can never hold what its
  parent lacked (proven adversarially: a parent without `gui` requesting `gui`
  for its child yields a child whose gui calls are denied).
- Trusted kernel-shell/selftest launches use a LEGACY_FULL set so V0.2–V0.6
  behavior is unchanged; **platform launches are default-deny** — an app gets
  only what its manifest requests and its launcher holds.

## Decision — filesystem sandbox

- A process may carry allowed path prefixes; the new `fs_read` syscall checks
  the **normalized** path component-wise against them
  (`kernel_core::path::is_within`), so `..` traversal, `//`, and
  component-boundary tricks (`/apps/hel` vs `/apps/hello`) cannot escape.
  Platform apps see `/apps/<name>` + `/etc` only. Out-of-sandbox reads are
  `ERR_PERM` + audited; in-sandbox misses stay `ERR_NOENT`.

## Decision — system services (no hidden daemons)

- Services are ordinary Ring 3 processes declared in a static registry:
  binary, dependencies, and the **exact** capabilities they run with. The
  supervisor computes a deterministic, cycle-checked startup order (Kahn's
  algorithm in host-tested `kernel_core::service`), co-schedules them
  preemptively, contains crashes, and applies a bounded restart policy
  (3 restarts, then `Failed` — no restart storms). Every transition emits a
  structured `[ITISYOU:SVC]` marker; `svc` shows real state.

## Decision — application/package model (ITPKG)

- An app = manifest (name, version, **requested caps**) + ELF payload in one
  ITPKG file, integrity-protected by SHA-256 over the content. Parsing is
  strict: exact lengths, no trailing bytes; unknown manifest keys, unknown
  capabilities, and duplicates are **errors** (a typo must never become an
  unreviewed grant).
- Launch re-verifies the digest, then runs the payload with
  `manifest ∩ launcher` capabilities under the app sandbox — request →
  capability check → deterministic service → action → audit.
- **Integrity ≠ authenticity**: SHA-256 catches corruption/tampering of the
  stored artifact but is not a signature. Signing needs key provisioning and
  a root of trust the platform does not yet have; the header reserves the
  digest slot the signature scheme will authenticate. Deferred, documented.

## Decision — atomic updates, rollback, recovery

- Store layout on persistent ITFS: `<app>.<v>.pkg` + `<app>.<v>.ok`; the
  active version is the highest `v` with both. ITFS gained atomic `remove`;
  every transition is **one crash-atomic double-buffered superblock commit**:
  - install/update: write `.pkg` (staged) → verify → create `.ok` (commit)
  - rollback: remove the newest `.ok`; the previous version reactivates and
    the demoted package remains as forensic evidence
  - recovery: a `.pkg` without `.ok` (interrupted update) is detected,
    reported, audited, and removed — it can **never** activate
- Proven across a real reboot: boot 1 installs v1 + stages v2 then powers
  off; boot 2 finds v1 active, recovers the orphan, launches v1.
- Kernel-image updates are out of scope: the OS does not own its boot media
  in the QEMU harness; the same staged/commit/rollback architecture is the
  design for it (V0.8+).

## Decision — audit / provenance

- Every privileged platform action and every capability denial is recorded:
  seq, tick, pid, action, capability, result (+ short detail — never payload
  contents), in a bounded ring mirrored to `[ITISYOU:AUDIT]` serial markers.
  The `audit` command shows the trail. Any future AI agent's requests flow
  through the same syscall/capability/service path and inherit this trail
  automatically. No AI component exists in the kernel.

## Alternatives

- **Capability handles (object capabilities)** instead of bits: the end goal;
  bits are the smallest enforceable step and the IPC layer is already shaped
  for handles (ADR-0007). Revisit when services own resources.
- **A general-purpose init/PID-1 in userspace**: the supervisor is kernel-side
  today because process lifecycle APIs are kernel APIs; the registry/policy
  split keeps the move to a userspace init tractable.
- **Content-addressed store / full package manager**: deliberately not built;
  the smallest deterministic, auditable install/update/rollback cycle first.

## Consequences

- Verified (QEMU selftest 106/0 + dedicated shell legs): default deny across
  7 syscall classes, sandbox traversal defense, no-amplification delegation,
  bounded service restart, package integrity refusals (corrupt + hostile
  manifest), atomic update/rollback, reboot-surviving recovery, audit trail.
- Limitations: capability bits not handles; no revocation of a running
  process's caps; sandbox covers fs_read (userspace has no write syscall
  yet); services are oneshot/bounded loops (no long-running background
  services while the shell polls serial); packages unsigned (integrity
  only); audit ring is in-memory (persisted only as serial evidence).
  Tracked in KNOWN_LIMITATIONS.
