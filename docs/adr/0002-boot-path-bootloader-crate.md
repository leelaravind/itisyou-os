# ADR-0002: Boot via the rust-osdev `bootloader` crate (BIOS + UEFI)

**Status:** Accepted · 2026-09-02

## Context

The plan (§6, §9) requires a maintained modern boot path with memory-map and
framebuffer handoff, UEFI via OVMF where practical, and a build that is
straightforward on a **Windows host**. Candidates: Limine, the rust-osdev
`bootloader` crate, GRUB/Multiboot2, or a custom UEFI loader.

## Decision

Use the rust-osdev `bootloader` crate (v0.11 line): the kernel links
`bootloader_api` and receives a typed `BootInfo` (memory regions, physical
memory mapping offset, framebuffer, RSDP); `tools/image-builder` uses the
host-side `bootloader` crate to produce **both BIOS and UEFI disk images in
pure Rust** — no xorriso/mtools/WSL requirement on Windows.

## Alternatives

- **Limine** — excellent protocol and handoff, but image assembly needs
  xorriso/guestfs-style tooling that is awkward and less reproducible on a
  native Windows host.
- **GRUB/Multiboot2** — 32-bit handoff (we'd own the long-mode transition),
  heavier tooling, legacy-leaning.
- **Custom UEFI loader (`uefi` crate)** — maximal control but significant
  scope added to V0.1 for no verification gain; revisitable later.

## Consequences

- An internal boot abstraction wraps `bootloader_api::BootInfo` so kernel
  subsystems are not coupled to third-party structures (plan §9.1).
- Both BIOS and UEFI images are built and both are exercised by the QEMU
  test matrix; UEFI (OVMF) is the primary documented path.
- The bootloader dependency is trusted boot-time-only code; it is not part
  of the OS runtime.
