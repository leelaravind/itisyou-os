# ADR-0001: Kernel implemented in Rust (`no_std`)

**Status:** Accepted · 2026-09-02

## Context

The V0.1 target is an independently bootable x86_64 kernel developed by an
autonomous agent on a Windows host, verified in QEMU, with a hard requirement
for memory-safety discipline, testability, and a minimal trusted computing
base (implementation plan §5, §6).

## Decision

Implement the kernel in Rust (`no_std`, target `x86_64-unknown-none`), with
assembly only where architecture entry/transitions require it. Pin a nightly
toolchain in `rust-toolchain.toml` (needed for artifact dependencies,
per-package targets, and later `abi_x86_interrupt`).

## Alternatives

- **C/C++** — mature OS pedigree but reintroduces whole classes of memory
  errors the project's security model is trying to design out; weaker
  host-side unit-testing story for pure logic.
- **Zig** — attractive comptime/freestanding support, but a smaller bare-metal
  ecosystem and no established bootloader handoff crates comparable to
  rust-osdev's.

## Consequences

- `unsafe` blocks are localized and must be documented (tracked for
  `UNSAFE_INVENTORY`, requirement SEC-001).
- Pure algorithmic logic lives in `crates/kernel-core`, compiled for host
  tests and for the kernel unchanged — the same code is unit-tested on the
  host and executed in QEMU.
- Nightly pinning is a reproducibility anchor; version recorded in
  `rust-toolchain.toml` and docs.
