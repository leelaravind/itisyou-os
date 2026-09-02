# Known limitations — honest current state

Updated continuously; last update: 2026-09-02 (session 1, V0.5 graphics +
input + desktop verified).

- QEMU is the only supported execution environment. Physical hardware boot
  is intentionally out of scope and untested.
- Scheduling (V0.4): **preemptive**, round-robin, no priorities; the quantum
  is fixed (2 ticks = 20 ms); there is no CPU accounting/fairness beyond
  round-robin. Registers and address-space isolation are preserved across
  preemption (machine-verified).
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
  a fully verified firmware path. Root cause not yet isolated.
- Graphics (V0.5): a single **bootloader-chosen framebuffer mode** (QEMU:
  1280×720 BGR); no mode-setting, no double-buffered vsync/flip, software
  rendering only, **no GPU acceleration** (Intel Iris Xe / virtio-gpu are out
  of scope). One 8×8 bitmap font, no scaling beyond integer, no Unicode.
- Compositor (V0.5): fixed wallpaper + top bar; **no window focus, drag,
  resize, z-order UI, or minimise/close controls**; windows are drawn
  back-to-front in creation order. Ownership + bounds are enforced (a process
  cannot draw into another window or out of bounds). The **live** desktop
  renders kernel-owned windows; compositing a *persistent Ring 3* window onto
  the live desktop needs asynchronous process spawning and is deferred —
  Ring 3 window rendering + isolation is proven in the selftest (a userspace
  process's window colour is read back off the composited screen).
- Input (V0.5): PS/2 **keyboard (IRQ1)** and **mouse (IRQ12)** via the i8042
  controller. Keyboard is scancode set 1, US layout, printable keys + a few
  controls; no key-repeat policy, no IME, extended keys carry no ASCII. Mouse
  is relative 3-byte packets (no scroll wheel). The **first** mouse packet
  after enabling reporting can carry a benign sync artifact (decoded as a
  zero-motion event); the decoder resynchronises and every subsequent packet
  decodes exactly. The IRQ handlers assume QEMU's IRQ routing rather than
  reading the i8042 aux-data status bit (adequate for the emulated device).
- The interactive serial shell still reads **polled serial** (the verified
  automation path); PS/2 keyboard drives the graphical desktop, not the serial
  shell.
- Syscalls run with interrupts masked; SMEP/SMAP are not enabled (absent on
  the QEMU CPU model); the user stack has an unmapped hole below it rather
  than a hardened guard region; CR3 switches do a full TLB flush (no
  PCID/ASID tagging).
- No networking, no USB/audio/Wi-Fi.
- Single CPU only; no SMP. Synchronization assumes one core; spinlocks are
  interrupt-safe by masking, not SMP-safe.
- Physical memory above 4 GiB is ignored by the frame allocator (counted
  and reported as `ignored_high_frames`); QEMU test configuration is 256 MiB.
- The PIC/PIT (legacy) interrupt path is the baseline; APIC/HPET are future
  work. Unmasked IRQs: 0 (timer), 1 (keyboard), 2 (cascade), 12 (mouse).
- The kernel heap is a fixed **32 MiB** range (grown from 1 MiB in V0.5 for
  the compositor back buffer + window stores); no growth, no guard pages on
  task stacks or the double-fault IST stack yet.
- The toolchain is pinned to nightly-2026-08-01 because newer nightlies
  break the bootloader crate's UEFI stage (rust-osdev/bootloader#579);
  the pin can move forward once upstream fixes land.
- Host tooling scripts are Windows-specific (`scripts/*.ps1`); CI provides
  the Linux path.
- Website deployment state is tracked in `docs/REQUIREMENTS.md` (CF-001).
