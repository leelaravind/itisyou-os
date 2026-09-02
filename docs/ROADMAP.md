# Roadmap — no fabricated dates

Milestones are sequenced by dependency, not calendar. A milestone is reached
when its verification gates pass, never before.

## V0.1 — Kernel Foundation *(complete — tag v0.1.0)*

Independently bootable x86_64 kernel in QEMU: serial diagnostics, physical +
virtual memory, heap, exceptions/interrupts/timer, kernel tasks + scheduler,
VFS/initramfs, interactive shell, automated boot/regression testing,
reproducible builds.

## V0.2 — Userspace Foundation *(current)*

Achieved so far: Ring 3 execution of real ELF64 programs, strict loader
(W^X, window, overlap policy), syscall ABI (write/exit/yield/getpid),
kernel/user MMU isolation, crash containment, clean process teardown.
Remaining in this milestone: per-process page tables (concurrent
processes), fork/exec-style process creation, IPC foundations, richer
syscalls.

## V0.3 — Storage

PCI enumeration maturity, block device abstraction, AHCI/NVMe research,
persistent filesystem strategy, crash-consistency experiments.

## V0.4 — Networking

NIC driver (VM target first), Ethernet, ARP/NDP, IPv4/IPv6 foundations,
UDP/TCP, DNS, strict network permission model.

## V0.5 — Graphics & Desktop Foundations

Framebuffer/display abstraction, input subsystem, compositor, windowing
primitives, text rendering, basic GUI toolkit, accessibility architecture.

## V0.6 — Hardware Expansion

USB, audio, device manager, ACPI/power foundations, laptop hardware
research, targeted driver strategy.

## V0.7 — System Platform

Service manager, package/runtime model, signed/atomic updates, sandboxing,
capability/permission engine, recovery and rollback.

## V0.8 — AI-Native System Layer

Local inference service **outside** the kernel, system knowledge over
approved local state, diagnostic agent, policy-controlled system actions
with preview/approval, provenance/audit, post-action verification.

## V0.9 — Daily-Driver Research

Wi-Fi, Bluetooth, accelerated graphics strategy, power management,
suspend/resume, application ecosystem — real hardware only on explicitly
approved sacrificial devices.

## V1.0 — Experimental Personal OS

Declared only when reliability, security, recovery, hardware support,
installation and update safety criteria are defined and passed.
