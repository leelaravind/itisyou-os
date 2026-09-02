# Architecture — ITISYOU OS V0.1

This document describes what exists now and the boundaries later milestones
build inside. Anything marked *(planned)* is design intent, not implemented
functionality.

## System shape

```
┌───────────────────────────────────────────────────────────┐
│ firmware (SeaBIOS / OVMF-UEFI, inside QEMU)               │
└──────────────┬────────────────────────────────────────────┘
               │ rust-osdev bootloader (BIOS & UEFI stages)
               ▼
┌───────────────────────────────────────────────────────────┐
│ kernel (Rust no_std, x86_64, ring 0)                      │
│                                                           │
│  boot handoff (BootInfo) → early serial → CPU baseline    │
│  → memory map → PMM → paging → heap → GDT/TSS/IDT         │
│  → PIC/PIT timer → scheduler → VFS/initramfs → shell      │
│    (stages B010 … B150, serial-observable)                │
└───────────────────────────────────────────────────────────┘
   userspace (ring 3, syscalls)          … stretch (planned)
   privileged services / capability engine … future (planned)
   AI layer (intelligence, never authority) … future (planned)
```

## Crate boundaries

- **`kernel/`** — the only privileged code. Library + two binaries:
  `itisyou-kernel` (interactive) and `itisyou-kernel-selftest`
  (boots, runs in-kernel checks, exits QEMU with a deterministic status).
- **`crates/kernel-core`** — pure logic with zero I/O: boot-stage contract,
  serial-marker grammar, and (as subsystems land) memory-map normalization,
  path handling, parsers. Compiled unchanged into both the kernel and host
  tools, unit-tested on the host.
- **`tools/image-builder`** — host tool; turns kernel ELFs into bootable
  BIOS/UEFI disk images (pure Rust) + SHA-256 manifest.
- **`tools/qemu-runner`** — host test harness; launches QEMU, asserts
  boot-stage markers, classifies panic/timeout/selftest outcomes, writes
  JSON evidence. Absence of output is never success.

## Boot contract

The bootloader hands the kernel a typed `BootInfo` (memory regions, physical
memory mapping offset, framebuffer, RSDP). Kernel code consumes it through
`itisyou_kernel::early_init` so subsystems never depend on third-party boot
structures directly. Boot stages B000–B150 (defined in
`kernel-core::stage`) each emit one machine-parseable serial marker; the
harness asserts them in order.

## Observability

Serial (COM1) is the primary channel and works before any allocator exists.
Marker grammar: `[ITISYOU:<TAG>] payload` with tags `B###`, `PANIC`,
`SELFTEST`, `TEST`, `INFO`, `MODE` — single source of truth in
`kernel-core::marker`, shared by emitter and asserter.

## Security posture (V0.1 reality)

- Everything runs ring 0 inside QEMU; isolation research starts at the
  *boundary definitions*, not premature claims. See `docs/SECURITY_MODEL.md`.
- Panic on violated invariants; panics emit `[ITISYOU:PANIC]` and, in test
  mode, fail the QEMU run deterministically.
- The AI-authority principle (intelligence ≠ authority) constrains future
  interfaces: kernel APIs are designed so privileged operations can later sit
  behind a deterministic policy/service layer rather than being callable by
  an agent directly. No AI component exists in the V0.1 runtime.

## Planned next boundaries

Physical memory manager → paging abstraction → heap → descriptors/interrupts
→ scheduler → VFS/initramfs → shell (dependency order, plan §20 Phase 2).
Each lands with host tests and QEMU selftests before the next begins.
