# ADR-0007: Minimal IPC — bounded kernel message channels

**Status:** Accepted · 2026-09-02 (V0.3)

## Context

The goal asks for "the smallest secure primitive useful for future system
services" while preserving the capability/least-authority direction and not
over-engineering.

## Decision

A small fixed set (4) of kernel-owned message channels. `msg_send(ch, ptr,
len)` copies a length-delimited byte message (≤256 B) from validated user
memory into a bounded per-channel queue (≤8 messages); `msg_recv(ch, ptr,
cap)` copies the oldest message out, or returns `ERR_AGAIN` when empty /
`ERR_2BIG` when the buffer is too small (message left queued). Non-blocking.

Copy-through-kernel (no shared user memory), the send/recv split, and bounded
queues are the exact shape a later capability handle will wrap — a channel id
is a stand-in for a future capability. No ambient authority beyond knowing a
channel id.

## Alternatives

- **Shared-memory IPC**: faster but exposes cross-process memory, cutting
  against the isolation just built.
- **Synchronous rendezvous (send blocks for a receiver)**: more powerful but
  needs blocking/wakeup wiring that the wait() path only just introduced;
  deferred to keep V0.3 minimal.

## Consequences

- Messages are bounded and copied, so a hostile sender cannot exhaust memory
  or alias a receiver's pages.
- Channels are global (not yet capability handles) — the V0.8 capability
  engine will replace the integer id with an unforgeable handle. Documented
  as direction, not a current control.
