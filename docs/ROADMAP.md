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

## V0.5 — Graphics + Input + Basic Desktop/Compositor *(complete — tag v0.5.0)*

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

## V0.6 — Hardware Expansion *(complete — tag v0.6.0)*

Achieved: a generic device/driver model (probe/bind, queryable device table)
over an enriched PCI foundation (BAR sizing, capability-list walking with
MSI/MSI-X/PCIe/PM detection, adversarial malformed handling); a **USB UHCI**
host controller with full device enumeration over control transfers and
**verified HID keyboard input** off the interrupt endpoint; an **AC97** audio
driver whose generated PCM samples are proven to traverse the driver → codec →
output path (captured to a WAV); a **unified input subsystem** (PS/2 + USB HID
behind one event stream, delivered to the GUI); hardware discovery via `lsdev`
(kernel + a Ring 3 tool); and a `devinfo` syscall giving userspace device
access without hardware authority. Drivers are polled, keeping the verified PIC
timer/PS-2 interrupt path intact. Remaining for a fuller platform: **USB xHCI**,
multi-device USB enumeration, bulk/isochronous transfers, audio capture,
APIC/IOAPIC/MSI interrupt routing, and ACPI/power foundations.

## V0.7 — System Platform *(current)*

Achieved: an explicit **capability model** (default deny at the syscall
boundary; delegation only narrows — a child can never hold what its parent
lacked); a **filesystem sandbox** (normalized-path prefix checks that defeat
traversal); a **service supervisor** over ordinary Ring 3 processes
(deterministic cycle-checked startup order, states, crash containment,
bounded restart policy, structured diagnostics — no hidden privileged
daemons); a managed **application model** (strict manifests requesting
capabilities; launches granted `manifest ∩ launcher` under an app sandbox);
the **ITPKG package** format (SHA-256-verified at install AND launch;
malformed/corrupt/hostile-manifest packages refused); **atomic updates with
rollback and recovery** on the persistent store (staged → one crash-atomic
commit; an interrupted update can never activate — proven across a real
reboot); and an **audit/provenance trail** for every privileged action and
denial. Future AI agents can only request through this same
capability/policy/service path (intelligence ≠ authority). Remaining for a
fuller platform: capability handles + revocation, signed packages (key
provisioning), userspace FS writes, long-running background services,
kernel-image updates.

## V0.8 — Networking, Authenticity & Hardening (delivered)

Delivered and verified: an e1000 driver with polled descriptor rings; ARP,
IPv4, ICMP echo (client and responder), UDP and DNS; capability-scoped Ring 3
sockets; Ed25519 package signatures with a compiled-in trust root;
capability-scoped userspace filesystem writes with atomic overwrite; the local
APIC, I/O APIC and MSI-X with real interrupt-delivery evidence; xHCI
enumeration and HID input; a hash-chained audit trail that survives reboots and
detects tampering; and SMEP/SMAP/UMIP alongside the existing W^X and stack
guard.

Deliberately NOT delivered, and recorded as such rather than reworded: **TCP**,
IPv6, DHCP, and routing beyond a single gateway. The I/O APIC is programmed but
its entries stay masked — line IRQs remain on the verified PIC path.

## V0.9 — Transport, Interrupt Cutover & Key Management

TCP with a real retransmission timer and connection state machine; DHCP and
IPv6 foundations; moving line-based IRQs onto the I/O APIC (with ACPI MADT
parsing) so the PIC can be retired; a signing key that never enters the source
tree, with rotation and revocation; signing the audit chain head; ACPI/power
foundations, additional device classes, laptop hardware research.

## V0.10 — AI-Native System Layer

Local inference service **outside** the kernel, system knowledge over
approved local state, diagnostic agent, policy-controlled system actions
with preview/approval, provenance/audit, post-action verification.

## V0.11 — Daily-Driver Research

Wi-Fi, Bluetooth, accelerated graphics strategy, power management,
suspend/resume, application ecosystem — real hardware only on explicitly
approved sacrificial devices.

## V1.0 — Experimental Personal OS

Declared only when reliability, security, recovery, hardware support,
installation and update safety criteria are defined and passed.
