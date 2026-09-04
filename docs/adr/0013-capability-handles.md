# ADR-0013: Resource-scoped capability handles

## Status

Accepted for V0.8 implementation; kernel/QEMU integration is pending.

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
It must not be described as runtime-verified until kernel and QEMU evidence is
added.
