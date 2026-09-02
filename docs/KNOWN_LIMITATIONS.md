# Known limitations — honest current state

Updated continuously; last update: 2026-09-02 (session 1, foundation phase).

- The kernel currently reaches early boot stages only (entry, serial, CPU
  baseline). Memory management, interrupts, scheduler, filesystem, and shell
  are **in development or planned** — see `docs/REQUIREMENTS.md` for the
  per-requirement state with evidence.
- QEMU is the only supported execution environment. Physical hardware boot
  is intentionally out of scope for V0.1 (plan §27) and untested.
- No userspace, no syscalls, no process isolation yet (stretch milestone).
- No persistent storage, no networking, no graphics beyond the
  bootloader-provided framebuffer (unused so far), no USB/audio/Wi-Fi.
- Single CPU only; no SMP. Synchronization primitives assume one core with
  interrupt masking.
- The PIC/PIT (legacy) interrupt path is the V0.1 baseline; APIC/HPET are
  future work.
- Boot images are currently **BIOS-only**: the bootloader crate's UEFI stage
  fails to link on current Rust nightlies (upstream
  rust-osdev/bootloader#579, `undefined symbol: wcslen`). QEMU boots the
  kernel via SeaBIOS; the UEFI/OVMF path is tracked as blocked-upstream in
  `docs/REQUIREMENTS.md` (BOOT-003) and will be restored when upstream fixes
  land or a compatible pinned nightly is identified.
- Host toolchain is Windows-specific in its scripts (`scripts/*.ps1`); CI
  provides the Linux build path.
- Website deployment depends on Cloudflare authentication being available;
  its status is tracked in `docs/REQUIREMENTS.md` (CF-001).
