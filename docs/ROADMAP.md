# Roadmap — no fabricated dates

Milestones are sequenced by dependency, not calendar. A milestone is reached
when its verification gates pass, never before.

## V0.1 — Kernel Foundation *(complete — tag v0.1.0)*

Independently bootable x86_64 kernel in QEMU: serial diagnostics, physical +
virtual memory, heap, exceptions/interrupts/timer, kernel tasks + scheduler,
VFS/initramfs, interactive shell, automated boot/regression testing,
reproducible builds.

## V0.2 — Userspace Foundation *(complete — tag v0.2.0)*

Ring 3 execution of real ELF64 programs, strict loader (W^X, window, overlap
policy), syscall ABI (write/exit/yield/getpid), kernel/user MMU isolation,
crash containment, clean process teardown.

## V0.3 — Process Isolation + Storage Foundation *(complete — tag v0.3.0)*

Per-process page tables with verified cross-process isolation; concurrent
Ring 3 processes (cooperative), spawn/wait/exit; strict user-buffer
validation; a minimal IPC channel; PCI enumeration; a block-device
abstraction; a read-only NVMe driver.

## V0.4 — Preemptive Multitasking + Persistent Storage *(complete — tag v0.4.0)*

Timer-driven preemptive scheduling (non-yielding processes cannot monopolize;
registers + address spaces preserved across preemption); NVMe write + flush;
ITFS, a minimal persistent filesystem with double-buffered CRC-committed
superblocks and verified reboot persistence.

## V0.5 — Graphics + Input + Basic Desktop/Compositor *(current)*

Achieved: a real graphical environment produced by the OS inside QEMU — a
bootloader-provided linear framebuffer wrapped with a `u32` back buffer and an
8×8 bitmap font; a window compositor with per-window backing stores and
ownership/bounds isolation (a process cannot draw outside its own window);
GUI syscalls so Ring 3 renders a window with no direct framebuffer access; PS/2
keyboard (IRQ1) and mouse (IRQ12) input; and an interactive desktop that reacts
to real keyboard + mouse events. Verified by an in-kernel selftest with pixel
read-backs (including a Ring 3 window's colour read off the composited screen)
and by a QEMU-monitor input-injection test that also screendumps the OS-rendered
desktop. Remaining for a richer desktop (V0.6+): mode-setting, window
focus/drag/z-order, compositing a persistent Ring 3 window on the live desktop,
a broader GUI toolkit, and accessibility architecture.

## V0.6 — Networking

NIC driver (VM target first), Ethernet, ARP/NDP, IPv4/IPv6 foundations,
UDP/TCP, DNS, strict network permission model.

## V0.7 — Hardware Expansion

USB, audio, device manager, ACPI/power foundations, laptop hardware
research, targeted driver strategy.

## V0.8 — System Platform

Service manager, package/runtime model, signed/atomic updates, sandboxing,
capability/permission engine, recovery and rollback.

## V0.9 — AI-Native System Layer

Local inference service **outside** the kernel, system knowledge over
approved local state, diagnostic agent, policy-controlled system actions
with preview/approval, provenance/audit, post-action verification.

## V0.10 — Daily-Driver Research

Wi-Fi, Bluetooth, accelerated graphics strategy, power management,
suspend/resume, application ecosystem — real hardware only on explicitly
approved sacrificial devices.

## V1.0 — Experimental Personal OS

Declared only when reliability, security, recovery, hardware support,
installation and update safety criteria are defined and passed.
