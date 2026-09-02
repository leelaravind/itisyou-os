# ADR-0005: Per-process address spaces

**Status:** Accepted · 2026-09-02 (V0.3)

## Context

V0.2 ran one process in the shared kernel address space. Concurrent processes
require memory isolation *between* user processes, not just user↔kernel.

## Decision

Each process owns a private level-4 page table ([`AddressSpace`]):

- **Kernel sharing by reference**: L4 entries 1..512 are copied from the boot
  L4, so every process sees identical supervisor-only kernel mappings. Ring 3
  can never reach them (U/S bit).
- **Private user window**: L4 entry 0 (0..512 GiB) is the process's exclusive
  subtree. Two processes share no user table frames, so the same user vaddr
  maps to different physical frames — structural isolation.
- **Loading without CR3 switches**: segments are copied through the
  physical-memory alias (`phys_to_virt`), so a process is fully built while
  another space is active.
- **Switching**: `activate_l4` writes CR3 around each quantum; the run-loop
  restores the kernel L4 on return.
- **Teardown**: the entry-0 subtree is walked recursively, returning every
  leaf and intermediate table frame plus the L4 to the PMM — leak-free by
  construction (verified by frame-exact selftests), no Vec bookkeeping.

The bootloader identity-maps handoff structures in L4 entry 0; these are
unlinked (`release_boot_identity_mappings`) at B080 once the kernel's own
GDT/IDT are live, so the user window is process-exclusive.

## Alternatives

- **Single space + per-process segment bookkeeping** (V0.2): cannot isolate
  processes from each other.
- **Recursive page-table mapping**: elegant but the offset-map alias already
  gives O(1) table access without a reserved slot.

## Consequences

- Pointer validation must use the ACTIVE CR3 (`translate_active`), not the
  kernel table — a V0.2 assumption that broke and was fixed here.
- MMIO for drivers is mapped in a dedicated kernel window shared via the
  kernel L4 entries, so it is visible in every process's space.
- ASID/PCID TLB tagging is not used (full CR3 reload flushes) — a documented
  performance limitation, not a correctness one.
