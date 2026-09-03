/**
 * Roadmap milestones — content from docs/ROADMAP.md ("no fabricated dates").
 * Milestones are sequenced by dependency, not calendar; a milestone is
 * reached when its verification gates pass, never before.
 *
 * Statuses use the allowed vocabulary honestly: V0.1–V0.5 are Verified
 * (machine-checked in QEMU); near-term milestones are Planned; the far
 * research milestones are Concept — documented direction, not active work.
 */
import type { ModuleStatus } from './status';

export interface Milestone {
  id: string;
  name: string;
  status: ModuleStatus;
  current?: boolean;
  summary: string;
  components: string[];
}

export const MILESTONES: Milestone[] = [
  {
    id: 'V0.1',
    name: 'Kernel Foundation',
    status: 'verified',
    summary:
      'Independently bootable x86_64 kernel in QEMU: serial diagnostics, physical + virtual memory, heap, exceptions/interrupts/timer, kernel tasks + scheduler, VFS/initramfs, interactive shell, automated boot/regression testing, reproducible builds.',
    components: [
      'Boot path (BIOS/UEFI via bootloader crate)',
      'Memory management (PMM, paging, heap)',
      'Interrupts, timer, scheduler, VFS, shell',
    ],
  },
  {
    id: 'V0.2',
    name: 'Userspace Foundation',
    status: 'verified',
    summary:
      'Ring 3 execution of real ELF64 programs, strict loader (W^X, window, overlap policy), syscall ABI (write/exit/yield/getpid), kernel/user MMU isolation, crash containment, clean process teardown.',
    components: ['Ring 3 + strict ELF64 loader (W^X)', 'Syscall ABI', 'Crash containment'],
  },
  {
    id: 'V0.3',
    name: 'Process Isolation + Storage Foundation',
    status: 'verified',
    summary:
      'Per-process page tables with verified cross-process isolation; concurrent Ring 3 processes (cooperative), spawn/wait/exit; strict user-buffer validation; a minimal IPC channel; PCI enumeration; a block-device abstraction; a read-only NVMe driver.',
    components: ['Per-process address spaces', 'Concurrency + IPC', 'PCI, block device, NVMe (read)'],
  },
  {
    id: 'V0.4',
    name: 'Preemptive Multitasking + Persistent Storage',
    status: 'verified',
    summary:
      'Timer-driven preemptive scheduling (non-yielding processes cannot monopolize; registers + address spaces preserved across preemption); NVMe write + flush; ITFS, a minimal persistent filesystem with double-buffered CRC-committed superblocks and verified reboot persistence.',
    components: ['Preemptive scheduler', 'NVMe write + flush', 'ITFS + reboot persistence'],
  },
  {
    id: 'V0.5',
    name: 'Graphics + Input + Basic Desktop/Compositor',
    status: 'verified',
    summary:
      'A real graphical environment produced by the OS inside QEMU: a bootloader framebuffer with a back buffer + bitmap font; a window compositor with per-window backing stores and ownership/bounds isolation; GUI syscalls so Ring 3 renders a window with no direct framebuffer access; PS/2 keyboard (IRQ1) + mouse (IRQ12); and an interactive desktop that reacts to real input. Proven by in-kernel pixel read-backs and a QEMU-monitor input-injection test that screendumps the OS-rendered desktop.',
    components: [
      'Framebuffer + compositor (ownership-isolated windows)',
      'GUI syscalls (Ring 3 rendering, no direct FB)',
      'PS/2 keyboard + mouse; interactive desktop',
    ],
  },
  {
    id: 'V0.6',
    name: 'Hardware Expansion',
    status: 'verified',
    summary:
      'A generic device/driver model over an enriched PCI foundation (BAR sizing, capability-list walking with MSI/MSI-X/PCIe/PM detection); a USB UHCI host controller with full device enumeration over control transfers and verified HID keyboard input; an AC97 audio driver whose generated PCM samples are proven to traverse driver -> codec -> output (captured to a WAV); a unified input subsystem (PS/2 + USB HID behind one event stream, delivered to the GUI); and a devinfo syscall giving userspace device access without hardware authority. Drivers are polled, keeping the verified PIC timer/PS-2 interrupt path intact.',
    components: [
      'Device/driver model + PCI depth (BARs, capabilities)',
      'USB UHCI + HID keyboard; AC97 audio (samples verified)',
      'Unified input; userspace device access (least authority)',
    ],
  },
  {
    id: 'V0.7',
    name: 'System Platform',
    status: 'verified',
    current: true,
    summary:
      'A coherent platform over the kernel: an explicit capability model enforced default-deny at the syscall boundary (delegation only narrows - a child can never hold what its parent lacked); a traversal-proof filesystem sandbox; a service supervisor over ordinary Ring 3 processes (deterministic cycle-checked startup order, crash containment, bounded restart policy - no hidden privileged daemons); applications as SHA-256-verified ITPKG packages launched with manifest-requested capabilities only; atomic updates with rollback and reboot-proven recovery (an interrupted update can never activate); and an audit trail recording every privileged action and every denial. Future AI agents can only request through this same capability/policy/service path.',
    components: [
      'Capabilities: default deny, no amplification (adversarially proven)',
      'Services, packages, atomic update/rollback/recovery',
      'Sandboxing + audit/provenance trail',
    ],
  },
  {
    id: 'V0.8',
    name: 'Networking',
    status: 'planned',
    summary:
      'NIC driver (VM target first), Ethernet, ARP/NDP, IPv4/IPv6 foundations, UDP/TCP, DNS, strict network permission model.',
    components: ['NIC driver (VM target first)', 'IPv4/IPv6, UDP/TCP, DNS', 'Strict network permission model'],
  },
  {
    id: 'V0.9',
    name: 'Advanced Hardware + Interrupt Modernization',
    status: 'planned',
    summary:
      'USB xHCI, APIC/IOAPIC/MSI interrupt routing, ACPI/power foundations, additional device classes, laptop hardware research, targeted driver strategy.',
    components: ['USB xHCI', 'APIC/IOAPIC/MSI', 'ACPI/power foundations'],
  },
  {
    id: 'V0.10',
    name: 'AI-Native System Layer',
    status: 'concept',
    summary:
      'Local inference service outside the kernel, system knowledge over approved local state, diagnostic agent, policy-controlled system actions with preview/approval, provenance/audit, post-action verification.',
    components: [
      'Local inference service (outside the kernel)',
      'Policy-controlled actions with preview/approval',
      'Provenance, audit, post-action verification',
    ],
  },
  {
    id: 'V0.11',
    name: 'Daily-Driver Research',
    status: 'concept',
    summary:
      'Wi-Fi, Bluetooth, accelerated graphics strategy, power management, suspend/resume, application ecosystem — real hardware only on explicitly approved sacrificial devices.',
    components: ['Wi-Fi & Bluetooth', 'Power management, suspend/resume', 'Sacrificial-device hardware research only'],
  },
  {
    id: 'V1.0',
    name: 'Experimental Personal OS',
    status: 'concept',
    summary:
      'Declared only when reliability, security, recovery, hardware support, installation and update safety criteria are defined and passed.',
    components: ['Declared by criteria, never by calendar'],
  },
];
