# ADR-0013: Resource-scoped capability handles

## Status

Accepted and verified for V0.8. Handles are the enforcement path on the syscall
boundary (`require_handle` in `kernel/src/syscall.rs`), not a parallel record.
(This status line read "integration is pending" until the 2026-09-13 audit,
which found it stale.)

## Decision

Extend the V0.7 static capability policy with opaque handles backed by a
bounded kernel-owned table. Each live entry binds an owner process, capability
kind, resource scope, rights, generation and optional expiry. A handle is only
an index plus generation; it is never authority by itself.

Delegation is explicit and can only reduce rights and scope. Revocation removes
the entry. Slot generations reject stale handles after reuse, and owner teardown
revokes every entry owned by that process.

## Consequences

The design is deterministic and usable in `no_std` kernel code without a heap.
Filesystem paths, device identifiers, service identifiers, and network port or
address ranges can be represented as resource scopes by their enforcing
service. Existing V0.1-V0.7 static bits remain as compatibility policy until
each syscall has a handle-based boundary and QEMU negative evidence.

## Evidence

The host-testable contract is implemented in
`crates/kernel-core/src/capability.rs`, including forged/stale handle,
ownership, scope, rights, delegation, revocation, teardown, and expiry tests.
Runtime evidence: QEMU `cap-handle-uefi` (a forged generation denied) and
`platform-bios`, where `cap-handle-probe` drives the real syscalls and a forged,
an expired and a revoked handle are each refused with the precise reason in the
audit trail (`CAPH-FORGED-DENIED`, `CAPH-EXPIRED-DENIED`, `CAPH-REVOKED-DENIED`,
`reason=expired`, `reason=no_handle`) — requirements CAPH-001/002. The V0.7
static bits no longer gate syscalls; they remain the request vocabulary for
manifests and `spawn_caps`.
