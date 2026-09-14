/**
 * Roadmap milestones — content from docs/ROADMAP.md ("no fabricated dates").
 * Milestones are sequenced by dependency, not calendar; a milestone is
 * reached when its verification gates pass, never before.
 *
 * Statuses use the allowed vocabulary honestly: delivered milestones are Verified
 * (machine-checked in QEMU); the milestone being built on `main` is In
 * Development until its release gate (CI on the release commit, then the tag)
 * has passed, even when every feature in it is already verified; near-term
 * milestones are Planned; the far research milestones are Concept — documented
 * direction, not active work.
 *
 * `current` marks the milestone of the branch this site is built from — the
 * one status/current.json names. Pages treat a current milestone that is not
 * yet Verified as "built but unreleased" and say its work is not in the
 * download, so `current` must never sit on a milestone whose work is not on
 * that branch (work on another branch is neither current nor released). V0.11
 * is current on its development branch, v0.11/work, whose status/current.json
 * names it; the text says "development branch" wherever that matters.
 */
import type { ModuleStatus } from './status';

export interface Milestone {
  id: string;
  name: string;
  status: ModuleStatus;
  current?: boolean;
  summary: string;
  components: string[];
  /**
   * Release state of an unreleased milestone — what still stands between its
   * verified work and a tagged release. Rendered on /roadmap, /releases and
   * /build; omit once the milestone is released.
   */
  release?: string;
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
    name: 'Networking, Authenticity & Hardening',
    status: 'verified',
    summary:
      'The first milestone whose inputs come from somewhere other than this machine. An e1000 driver with polled descriptor rings; ARP, IPv4, ICMP echo (answered as well as sent), UDP and DNS; Ring 3 sockets behind a capability scoped to a port. Packages are now authenticated as well as integrity-checked: Ed25519 signatures over a context, the declared lengths and the content digest, against a compiled-in trust root - an unsigned package, one signed by a stranger and one with a forged signature are three DIFFERENT refusals. Userspace can write to the persistent store under the write right and its sandbox, with an overwrite that is one crash-atomic commit. The local APIC, I/O APIC and MSI-X are up with real delivery evidence, xHCI enumerates and reads HID input, the audit trail is hash-chained and survives reboots, and SMEP/SMAP/UMIP now enforce kernel/user separation in the CPU rather than only in the page tables. NOT delivered, and said so: TCP, IPv6 and DHCP. v0.8.1 closed the release out: its kernel now names the version the tag does, and the build fails if the two ever drift again.',
    components: [
      'e1000 + ARP/IPv4/ICMP/UDP/DNS; port-scoped socket capability',
      'Ed25519 package signatures; userspace filesystem writes',
      'APIC/MSI-X, xHCI, hash-chained persistent audit, SMEP/SMAP/UMIP',
    ],
  },
  {
    id: 'V0.9',
    name: 'Transport, Interrupt Cutover & Trust',
    status: 'verified',
    summary:
      'Released as v0.9.0. TCP from Ring 3: a host-tested RFC 9293 state machine with a retransmission timer and a receive window; programs connect and listen (one connection per listen) under a network capability scoped to the port; verified against the host operating system’s own TCP stack, including two deliberately dropped segments recovered by retransmission. A DHCP client, and IPv6 foundations: link-local and SLAAC addresses, neighbour discovery, ICMPv6 echo in both directions. ACPI table discovery and the I/O APIC cutover that retires the 8259 PIC, plus ACPI S5 power-off. A package-signing key hierarchy: only an offline root is compiled in, signing keys are trusted through root-signed certificates limited by package-name scope and a release-epoch validity window, a root-signed revocation list retires them, and the private keys stay outside the source tree. The audit head anchored with a witness off the audited disk, and guard pages under the kernel’s Ring 3 interrupt stack (RSP0) and double-fault stack, so an overflow of either faults instead of silently corrupting kernel data (not yet under the separate syscall stack — the one the TCP bug overflowed; found after the release, fix planned for V0.10). Also fixed on the way: a program’s sockets and connections are released on every exit path, broadcast UDP is checksummed against its real destination, and a zero-length copy never touches the user pointer — before that fix any program could panic the kernel with cap_list(NULL, 0). Stated limits: no TCP over IPv6 and no IPv6 sockets, no TCP congestion control beyond an in-flight cap, ACPI tables without an AML interpreter, and no guard pages yet for the syscall stack or heap-allocated kernel task stacks.',
    components: [
      'TCP from Ring 3 (connect + listen), DHCP, IPv6 foundations',
      'ACPI + I/O APIC cutover (PIC retired), ACPI S5 power-off',
      'Offline-root signing-key hierarchy; audit anchoring; kernel stack guards',
    ],
  },
  {
    id: 'V0.10',
    name: 'Userspace System',
    status: 'verified',
    summary:
      'Released as v0.10.0. Background programs keep running whatever the console is doing: bounded slices at audited safe points (the kernel stays non-preemptible), measured per command. A process tree — only a parent collects its child, non-blocking wait_nohang, sleep, orphans handed on, ps and kill. /sbin/init runs as pid 1, starts and supervises the services named in /etc/init.conf with exactly the capabilities the file gives them, reports them through a kernel-checked record, and is restarted by the kernel if it dies. Program arguments, a Ring 3 shell that borrows the console’s input, filesystem space reclamation, and persistent Ring 3 applications on the live desktop with click-to-focus and keys routed to the focused window. Found and fixed on the way, each shown against a negative control: a denial of service in every earlier release (the kernel wrote through user pages the program could not write), the syscall stack V0.9 left unguarded, Ring 3 DF/AC flags reaching kernel code, a mouse-packet framing bug, and an interrupt-unsafe input queue. Stated limits: no kernel preemption (non-schedulable regions are measured and listed), configuration only in the boot image, a shell without pipes or job control, one app per desktop session.',
    components: ['Always-on scheduler + process tree', '/sbin/init from /etc/init.conf', 'Ring 3 shell; desktop apps with focus'],
  },
  {
    id: 'V0.11',
    name: 'AI-Native System Layer',
    status: 'in-development',
    current: true,
    summary:
      'Implemented and verified on the development branch — host tests and QEMU legs, each shown against a negative control — and not yet released. The agent has intelligence, not authority. The audit trail came first: three defects present since v0.8.0 were fixed before any agent code (a stored trail that read as TAMPERED once a later boot had saved it, a trail and package store that a program holding fs_write without a sandbox could rewrite, file names that could print forged kernel markers). Then the layer: a fixed, read-only view of the kernel’s own tables (sys_view, syscall 42), served only to a process the console grants a console-only capability; a small diagnostic model — three integer yes/no detectors trained at build time from SYNTHETIC scenarios and pinned by digest — whose held-out accuracy equals a one-rule baseline, so it shows the designed cases are separable, not that it diagnoses real machines; inferd, a Ring 3 service under /sbin/init that runs the model; one runbook per condition; and a diagnostic agent that can file proposals (propose, syscall 43) and do nothing else. The kernel checks every proposal against what it knows itself — the shipped model, the view it served that process, its own recomputation of the diagnosis, the system as it is now — and only the console can approve one. Two actions exist, both kernel code, each verified afterwards and rolled back if the check fails: resume-scheduler, and retry-service through a kernel→init mailbox (init_ctl, syscall 44). Process output can no longer drive the operator’s terminal. Stated limits: synthetic training data, three conditions and two actions, unauthenticated IPC (inferd’s answers are claims the kernel re-checks), proposals held only in memory.',
    components: [
      'Read-only system view; pinned diagnostic model; inferd in Ring 3',
      'Diagnostic agent with runbooks; kernel-checked proposals',
      'Console-only approval; kernel execution, verification, rollback',
    ],
    release:
      'Release pending. Every V0.11 requirement is implemented and verified on the development branch (v0.11/work), each with a negative control; what remains is the release itself — the branch brought onto main, a release commit naming 0.11.0, CI green on that commit (GitHub runs the jobs only during a short public window for the repository, as for v0.9.0 and v0.10.0), then the tag, the website and the download. Until then V0.11 is not released and not downloadable: the latest release, and the current download, is v0.10.0.',
  },
  {
    id: 'V1.0',
    name: 'Experimental Personal OS',
    status: 'concept',
    summary:
      'Declared only when acceptance criteria written in advance — reliability, security, recovery, update safety, boot safety and supported environment — are passed. Ships as a reproducible, checksummed, downloadable boot image for virtual machines; physical hardware is explicitly unsupported.',
    components: ['Declared by criteria, never by calendar', 'Reproducible, checksummed VM boot image'],
  },
  {
    id: 'Post-1.0',
    name: 'Daily-Driver Research',
    status: 'concept',
    summary:
      'Moved after V1.0 by the 2026-09-13 amendment: Wi-Fi, Bluetooth, GPU acceleration, power management and suspend/resume need physical hardware that QEMU cannot stand in for, and physical hardware needs an explicit per-device authorization gate. A requirement that cannot be verified cannot gate V1.0.',
    components: ['Wi-Fi & Bluetooth', 'Power management, suspend/resume, SMP', 'Sacrificial-device hardware research only'],
  },
];
