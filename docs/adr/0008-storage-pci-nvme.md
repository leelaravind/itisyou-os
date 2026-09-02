# ADR-0008: Storage foundation — PCI enumeration, block abstraction, read-only NVMe

**Status:** Accepted · 2026-09-02 (V0.3)

## Context

V0.3 begins storage. The goal: PCI enumeration hardening, a block-device
abstraction, and a real emulated-device driver (AHCI or NVMe), read-only
first, proven in QEMU, without risking physical disks.

## Decision

- **PCI enumeration** via the 0xCF8/0xCFC config mechanism (`device::pci`),
  with class/identity decoding in host-tested `kernel_core::pci`. Brute-force
  scan of bus 0 (QEMU's default topology); multifunction-aware.
- **Block abstraction** (`device::block`): a narrow read-only `BlockDevice`
  trait (`read_block`/`read_blocks`, 512-byte blocks) plus a deterministic
  RAM-backed `RamDisk` that proves the trait, read path, and error handling
  independently of any controller.
- **NVMe** chosen over AHCI: modern, well-emulated by QEMU, and a cleaner
  minimal bring-up. `device::nvme` maps BAR0 as uncacheable MMIO, resets the
  controller, sets up admin + one I/O queue pair (polled, no MSI-X),
  Identifies namespace 1 for the block count, and implements the Read
  command. DMA buffers are single PMM frames (physically contiguous), seen by
  the controller by physical address and by the kernel through the
  physical-memory alias.
- **Safety**: QEMU attaches a **generated disposable raw disk** (the runner's
  `--nvme` flag) whose LBA 0 carries a known magic; the kernel reads it back
  and asserts the magic. No host disk is ever touched. Read-only only; no
  writes, no persistent filesystem yet.

## Alternatives

- **AHCI/SATA**: also viable and named in the goal, but a heavier port
  (command list + FIS + ports) for the same read-only proof.
- **virtio-blk**: simplest of all, but the goal named AHCI/NVMe; NVMe is the
  more valuable long-term target.

## Consequences

- A real block read from an emulated controller is verified end-to-end
  (`nvme_disk_magic`), establishing the storage foundation.
- Polled I/O + single queue + no interrupts + read-only are explicit V0.3
  limitations; write support, MSI-X, and a persistent filesystem are V0.4+.
- The MMIO window is mapped in kernel L4 space (shared by every process), so
  drivers work regardless of the active address space.
