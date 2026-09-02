# Known limitations — honest current state

Updated continuously; last update: 2026-09-02 (session 1, kernel foundation
verified).

- QEMU is the only supported execution environment. Physical hardware boot
  is intentionally out of scope for V0.1 (plan §27) and untested.
- Scheduling is **cooperative** (explicit `yield`); the PIT timer accounts
  ticks but does not preempt yet. Timer-driven preemption is future work.
- Input is **polled serial only** — the verified automation path. PS/2
  keyboard support is tracked separately (`input-keyboard: planned`).
- Userspace (V0.2): **one user process at a time** in a shared address
  space — isolation user↔kernel is MMU-enforced (U/S bit, verified), but
  per-process page tables (isolation *between* user processes) are future
  work and required before concurrent processes. No fork/exec/IPC. Syscalls
  run with interrupts masked; SMEP/SMAP are not enabled (absent on the QEMU
  CPU model); the user stack has an unmapped hole below it rather than a
  hardened guard region.
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
