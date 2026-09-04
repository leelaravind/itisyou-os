# Known limitations — honest current state

Updated continuously; last update: 2026-09-03 (V0.7 release evidence closeout).

- Platform (V0.7): capabilities are **bits, not handles** — no revocation of
  a running process's authority, no per-resource capabilities yet (the IPC
  layer is shaped for handles; ADR-0007/0012). The FS sandbox governs
  `fs_read` only — userspace has **no filesystem write syscall** at all, so
  write confinement is moot until one exists. Packages are
  **integrity-verified (SHA-256), not signed** — authenticity needs key
  provisioning and a root of trust (deferred, ADR-0012). The audit ring is
  in-memory (64 records; serial markers are the durable evidence). Services
  are oneshot/bounded programs supervised to terminal states — there are no
  long-running background services while the shell polls serial, and no
  userspace init. Kernel-image updates are out of scope (the OS does not own
  its boot media in QEMU); the app store's staged/commit/rollback is the
  designed mechanism. ITFS `remove` does not reclaim data blocks (no
  free-block reuse — space leaks by design for crash-atomicity simplicity).

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
- Devices (V0.6): a generic device/driver model over PCI with two real class
  drivers (AC97 audio, UHCI USB), plus NVMe. **All drivers are polled** — no
  device IRQ is wired (the PIC path stays timer/PS-2 only). MSI/MSI-X
  capabilities are detected + reported but not used; APIC/IOAPIC/MSI routing is
  deferred. PCI enumeration scans bus 0 only (complete on the QEMU `pc`
  machine; no bridge recursion).
- USB (V0.6): **UHCI** (USB 1.1) only — no xHCI/EHCI/OHCI. Enumerates a single
  device (the first connected root port); no hubs, no multi-device addressing.
  Control + interrupt-IN transfers only (no bulk/isochronous). HID **boot
  protocol** keyboard input is verified; mouse decode is host-tested but full
  mouse-input verification needs multi-device enumeration.
- Audio (V0.6): **AC97 output only** — no capture/input, one PCM-out stream,
  fixed 48 kHz, no mixing/resampling in the OS (QEMU resamples to the backend).
- No networking, no Wi-Fi/Bluetooth.
- Single CPU only; no SMP. Synchronization assumes one core; spinlocks are
  interrupt-safe by masking, not SMP-safe.
- Physical memory above 4 GiB is ignored by the frame allocator (counted
  and reported as `ignored_high_frames`); QEMU test configuration is 256 MiB.
- The PIC/PIT (legacy) interrupt path is the baseline; APIC/HPET/MSI are future
  work. Unmasked IRQs: 0 (timer), 1 (keyboard), 2 (cascade), 12 (mouse). Device
  drivers (NVMe/AC97/UHCI) are polled, so they need no IRQ line.
- The kernel heap is a fixed **32 MiB** range (grown from 1 MiB in V0.5 for
  the compositor back buffer + window stores); no growth, no guard pages on
  task stacks or the double-fault IST stack yet.
- The toolchain is pinned to nightly-2026-08-01 because newer nightlies
  break the bootloader crate's UEFI stage (rust-osdev/bootloader#579);
  the pin can move forward once upstream fixes land.
- Host tooling scripts are Windows-specific (`scripts/*.ps1`); CI provides
  the Linux path.
- Website deployment state is tracked in `docs/REQUIREMENTS.md` (CF-001).

## V0.8 work in progress

The V0.8 capability-handle contract is currently host-tested only. It is not
yet wired into the kernel syscall boundary, so V0.7 static capability bits
remain the active runtime policy. Networking, userspace filesystem writes,
signed package authentication, long-running userspace services, APIC/MSI,
xHCI, and persistent audit storage remain unverified until corresponding
QEMU/CI evidence is produced.
