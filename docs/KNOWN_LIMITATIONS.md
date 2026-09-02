# Known limitations — honest current state

Updated continuously; last update: 2026-09-02 (session 1, kernel foundation
verified).

- QEMU is the only supported execution environment. Physical hardware boot
  is intentionally out of scope for V0.1 (plan §27) and untested.
- Scheduling is **cooperative** (explicit `yield`); the PIT timer accounts
  ticks but does not preempt yet. Timer-driven preemption is future work.
- Input is **polled serial only** — the verified automation path. PS/2
  keyboard support is tracked separately (`input-keyboard: planned`).
- Userspace (V0.4): **concurrent, preemptively-scheduled** Ring 3 processes,
  each in its own address space. The timer preempts non-yielding processes
  (2-tick quantum); registers and address-space isolation are preserved
  across preemption (machine-verified). Scheduling is round-robin with no
  priorities; the quantum is fixed; there is no CPU accounting/fairness
  beyond round-robin.
- Process model: spawn/wait/exit and a single blocking waiter per child;
  no fork/exec-with-args, no process groups, no signals yet.
- IPC: bounded kernel message channels (4 channels, ≤256 B, ≤8 queued),
  non-blocking, addressed by integer id (a future capability handle). No
  shared-memory or synchronous-rendezvous IPC yet.
- Storage (V0.4): NVMe **read + write + flush** (still polled, single I/O
  queue, single namespace, no MSI-X). ITFS is a minimal persistent
  filesystem: fixed directory (≤12 files), **contiguous append-only** files —
  no per-file update/delete/rename, no directories/subpaths, no free-block
  reuse. Crash consistency covers the metadata superblock (double-buffered
  CRC commit); a torn *data* write of an in-progress file is not journaled
  (the file is only referenced after its body is flushed). AHCI/virtio-blk
  not yet implemented (block trait is ready for it). QEMU attaches only
  generated disposable disks; no host disk is ever touched.
- The `itisyou-fs-persist` binary's **BIOS** disk image does not boot (a
  bootloader BIOS-stage quirk specific to that binary; its ELF is valid and
  its UEFI image boots). The reboot-persistence test therefore runs on UEFI,
  a fully verified firmware path. Root cause not yet isolated (V0.5 follow-up).
- Syscalls run with interrupts masked; SMEP/SMAP are not enabled (absent on
  the QEMU CPU model); the user stack has an unmapped hole below it rather
  than a hardened guard region; CR3 switches do a full TLB flush (no
  PCID/ASID tagging).
- The filesystem is a read-only in-memory initramfs; no persistent storage,
  no writes.
- No networking, no graphics output beyond the bootloader-provided
  framebuffer (present but unused), no USB/audio/Wi-Fi.
- Single CPU only; no SMP. Synchronization assumes one core; spinlocks are
  interrupt-safe by masking, not SMP-safe.
- Physical memory above 4 GiB is ignored by the frame allocator (counted
  and reported as `ignored_high_frames`); QEMU test configuration is 256 MiB.
- The PIC/PIT (legacy) interrupt path is the V0.1 baseline; APIC/HPET are
  future work. Only IRQ0 (timer) is unmasked.
- The kernel heap is a fixed 1 MiB range; no growth, no guard pages on task
  stacks or the double-fault IST stack yet.
- The toolchain is pinned to nightly-2026-08-01 because newer nightlies
  break the bootloader crate's UEFI stage (rust-osdev/bootloader#579);
  the pin can move forward once upstream fixes land.
- Host tooling scripts are Windows-specific (`scripts/*.ps1`); CI provides
  the Linux path.
- Website deployment state is tracked in `docs/REQUIREMENTS.md` (CF-001).
