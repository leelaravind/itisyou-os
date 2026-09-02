/**
 * Roadmap milestones — content from docs/ROADMAP.md ("no fabricated dates").
 * Milestones are sequenced by dependency, not calendar; a milestone is
 * reached when its verification gates pass, never before.
 *
 * Statuses use the allowed vocabulary honestly: only V0.1 is in development;
 * near-term milestones are Planned; the far research milestones (V0.8+) are
 * Concept — documented direction, not active work.
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
    status: 'in-development',
    current: true,
    summary:
      'Independently bootable x86_64 kernel in QEMU: serial diagnostics, physical + virtual memory, heap, exceptions/interrupts/timer, kernel tasks + scheduler, VFS/initramfs, interactive shell, automated boot/regression testing, reproducible builds.',
    components: [
      'Boot path (BIOS/UEFI via bootloader crate)',
      'Memory management (PMM, paging, heap)',
      'Interrupts, timer, scheduler, VFS, shell',
      'Stretch: ring 3 + minimal syscalls + ELF loading',
    ],
  },
  {
    id: 'V0.2',
    name: 'Userspace Foundation',
    status: 'planned',
    summary:
      'Stable process abstraction, ring 3 isolation, ELF loading, syscall ABI, IPC foundations, user programs, stronger memory protection.',
    components: ['Process abstraction & ring 3 isolation', 'ELF loading + syscall ABI', 'IPC foundations'],
  },
  {
    id: 'V0.3',
    name: 'Storage',
    status: 'planned',
    summary:
      'PCI enumeration maturity, block device abstraction, AHCI/NVMe research, persistent filesystem strategy, crash-consistency experiments.',
    components: ['PCI enumeration', 'Block device abstraction', 'Persistent filesystem strategy'],
  },
  {
    id: 'V0.4',
    name: 'Networking',
    status: 'planned',
    summary:
      'NIC driver (VM target first), Ethernet, ARP/NDP, IPv4/IPv6 foundations, UDP/TCP, DNS, strict network permission model.',
    components: ['NIC driver (VM target first)', 'IPv4/IPv6, UDP/TCP, DNS', 'Strict network permission model'],
  },
  {
    id: 'V0.5',
    name: 'Graphics & Desktop Foundations',
    status: 'planned',
    summary:
      'Framebuffer/display abstraction, input subsystem, compositor, windowing primitives, text rendering, basic GUI toolkit, accessibility architecture.',
    components: ['Display + input subsystems', 'Compositor & windowing primitives', 'Accessibility architecture'],
  },
  {
    id: 'V0.6',
    name: 'Hardware Expansion',
    status: 'planned',
    summary:
      'USB, audio, device manager, ACPI/power foundations, laptop hardware research, targeted driver strategy.',
    components: ['USB & audio', 'Device manager, ACPI/power foundations', 'Targeted driver strategy'],
  },
  {
    id: 'V0.7',
    name: 'System Platform',
    status: 'planned',
    summary:
      'Service manager, package/runtime model, signed/atomic updates, sandboxing, capability/permission engine, recovery and rollback.',
    components: ['Signed/atomic updates', 'Sandboxing + capability/permission engine', 'Recovery and rollback'],
  },
  {
    id: 'V0.8',
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
    id: 'V0.9',
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
