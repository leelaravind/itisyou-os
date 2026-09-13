/**
 * Tagged releases — facts from Git and GitHub Actions, not marketing.
 *
 * Every entry is a version tag whose commit has a green CI run on
 * ubuntu-24.04 (`gh run list --commit <sha>`). The repository is private, so a
 * tag is evidence of a verified state, not a public download: until a release
 * ships a published, checksummed artifact, `artifact` stays null and the site
 * says so. The newest entry is updated in the same change that stamps its
 * evidence into status/current.json.
 */
export interface Release {
  tag: string;
  commit: string;
  date: string;
  milestone: string;
  ciRun: string;
  summary: string;
  /** Public download, once one exists: file name, size, SHA-256, URL. */
  artifact: null | { file: string; bytes: number; sha256: string; url: string };
}

export const RELEASES: Release[] = [
  {
    tag: 'v0.8.1',
    commit: 'pending',
    date: '2026-09-13',
    milestone: 'V0.8 — release-integrity closeout',
    ciRun: 'pending',
    summary:
      'No new kernel function. Corrects v0.8.0, whose kernel reported 0.7.0-dev: the version is now consistent everywhere and a build gate fails on any future drift between the kernel, the status metadata and the requirement matrix. Stale public claims corrected; the site is now verified in a real browser.',
    artifact: null,
  },
  {
    tag: 'v0.8.0',
    commit: '84d6b24',
    date: '2026-09-05',
    milestone: 'V0.8 — Networking, Authenticity & Hardening',
    ciRun: '33988391309',
    summary:
      'e1000 + ARP/IPv4/ICMP/UDP/DNS with port-scoped Ring 3 sockets; Ed25519-signed packages; capability handles as the enforcement path; userspace filesystem writes; APIC/MSI-X; xHCI; persistent hash-chained audit; SMEP/SMAP/UMIP. Its kernel misreported its version (0.7.0-dev), corrected in v0.8.1.',
    artifact: null,
  },
  {
    tag: 'v0.7.0',
    commit: '00b5eea',
    date: '2026-09-03',
    milestone: 'V0.7 — System Platform',
    ciRun: '33812762322',
    summary:
      'Capability model (default deny, no amplification), filesystem sandbox, service supervisor, ITPKG packages with SHA-256 integrity, atomic update/rollback/recovery across reboots, audit trail.',
    artifact: null,
  },
  {
    tag: 'v0.6.0',
    commit: 'cccf5a7',
    date: '2026-09-02',
    milestone: 'V0.6 — Hardware Expansion',
    ciRun: '33635955858',
    summary: 'Device/driver model, PCI BARs and capabilities, UHCI + USB HID keyboard, AC97 audio output, unified input.',
    artifact: null,
  },
  {
    tag: 'v0.5.0',
    commit: '5be3c0c',
    date: '2026-09-02',
    milestone: 'V0.5 — Graphics + Input + Desktop',
    ciRun: '33628363046',
    summary: 'Framebuffer graphics, ownership-isolated compositor, GUI syscalls, PS/2 keyboard and mouse, interactive desktop.',
    artifact: null,
  },
  {
    tag: 'v0.4.0',
    commit: 'b1af235',
    date: '2026-09-02',
    milestone: 'V0.4 — Preemption + Persistent Storage',
    ciRun: '33619289190',
    summary: 'Timer-driven preemption of Ring 3, NVMe write + flush, ITFS with crash-consistent superblocks and reboot persistence.',
    artifact: null,
  },
  {
    tag: 'v0.3.0',
    commit: 'd83942b',
    date: '2026-09-02',
    milestone: 'V0.3 — Process Isolation + Storage Foundation',
    ciRun: '33609090873',
    summary: 'Per-process address spaces, concurrent processes, spawn/wait, IPC, PCI enumeration, read-only NVMe.',
    artifact: null,
  },
  {
    tag: 'v0.2.0',
    commit: 'ee8d57c',
    date: '2026-09-02',
    milestone: 'V0.2 — Userspace Foundation',
    ciRun: '33604047455',
    summary: 'Ring 3 ELF64 programs, strict loader with W^X, syscall ABI, crash containment.',
    artifact: null,
  },
  {
    tag: 'v0.1.0',
    commit: '5a4b008',
    date: '2026-09-02',
    milestone: 'V0.1 — Kernel Foundation',
    ciRun: '33599387686',
    summary: 'Bootable x86_64 kernel: memory management, interrupts, scheduler, VFS/initramfs, shell, QEMU test harness.',
    artifact: null,
  },
];
