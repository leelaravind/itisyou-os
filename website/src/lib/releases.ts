/**
 * Tagged releases — facts from Git and GitHub Actions, not marketing.
 *
 * Every entry is a version tag whose content commit has a green CI run on
 * ubuntu-24.04 (`gh run list --commit <sha>`); the tag itself sits on the
 * status-stamp commit that follows it. A tag is evidence of a verified state,
 * not a download: until a release ships a published, checksummed artifact,
 * `download` stays null and the site says so. The newest entry is updated in
 * the same change that stamps its evidence into status/current.json.
 *
 * /download renders the release in src/data/downloads.json. A release with
 * images also carries `contents` and `limitations` for that page; once
 * /download moves on to a newer release, the older one keeps its published
 * files under `archive` so it stays downloadable from /downloads/<tag>/.
 * Published files are immutable, so `archive` is a record of what was
 * published, not a second manifest.
 */
export interface ReleaseFile {
  file: string;
  firmware: string;
  bytes: number;
  sha256: string;
}

export interface Release {
  tag: string;
  commit: string;
  date: string;
  milestone: string;
  ciRun: string;
  summary: string;
  /** Site path of the release's public download, once one exists. */
  download: string | null;
  /** What the release's boot images contain, for /download. */
  contents?: string;
  /** The first limits a user of those images meets; the full list is docs/KNOWN_LIMITATIONS.md. */
  limitations?: string[];
  /** Published files of a release that /download no longer serves as the current one. */
  archive?: {
    builtBy: string;
    files: ReleaseFile[];
    notice?: string;
  };
}

export const RELEASES: Release[] = [
  {
    tag: 'v0.10.0',
    commit: 'c936bc7',
    date: '2026-09-13',
    milestone: 'V0.10 — Userspace System',
    ciRun: '34782271250',
    summary:
      'A userspace system on top of V0.9. Background programs keep running while the console is busy: bounded slices at audited safe points, with the kernel still non-preemptible. A process tree: only a parent collects its child, wait_nohang, sleep, orphans handed on, ps and kill. /sbin/init as pid 1 starts and supervises the services named in /etc/init.conf, reports them to a kernel-checked service table, and is restarted by the kernel if it dies. Program arguments, a Ring 3 shell that borrows the console’s input, filesystem space reclamation, and Ring 3 applications on the live desktop with click-to-focus and keys routed to the focused window. Fixed on the way: a denial of service in every earlier release (the kernel wrote through user pages the program could not write), guard pages for the syscall and kernel-task stacks, a mouse-packet framing bug, an interrupt-unsafe input queue, and a gui_present that checked existence instead of ownership. UEFI and BIOS boot images built by the release CI run on Linux, published with SHA-256.',
    download: '/download',
    contents:
      'V0.10 — Userspace System, on top of everything in V0.9 (TCP from Ring 3, DHCP, IPv6 foundations, the I/O APIC cutover, ACPI power-off, an offline-root package-signing hierarchy, audit anchoring): always-on co-scheduling at audited safe points (sched reports what the background got); a process tree with parent-only wait, wait_nohang, sleep, orphans, ps and kill; /sbin/init as pid 1 from /etc/init.conf, restarted by the kernel; a kernel-checked supervisor report (svc_report); program arguments; a Ring 3 shell (rsh at the console); ITFS space reclamation; Ring 3 desktop apps (desktop /bin/gui-echo) with click-to-focus and routed input; line-atomic Ring 3 output with unforgeable kernel markers; DF/AC cleared on every kernel entry; guard pages under the RSP0, double-fault, syscall and kernel-task stacks; user buffers the kernel writes must be writable by the program.',
    limitations: [
      'The kernel is not preemptible: background programs run in bounded slices at audited points, and wait while a syscall or a storage, irq or xhciwait command runs (measured up to 5 s for xhciwait).',
      '/etc/init.conf is part of the boot image: no persistent configuration, no reload.',
      'The Ring 3 shell has builtins and run — no pipes, redirection, job control or environment; administration stays on the kernel console.',
      'Desktop: one Ring 3 app per desktop session; click-to-focus without drag, resize or close.',
      'TCP is IPv4 only; no congestion control beyond an in-flight cap. The boot image itself is not authenticated.',
      'One CPU; software rendering only; no suspend/resume.',
    ],
  },
  {
    tag: 'v0.9.0',
    commit: 'f06673e',
    date: '2026-09-13',
    milestone: 'V0.9 — Transport, Interrupt Cutover & Trust',
    ciRun: '34760629701',
    summary:
      'TCP from Ring 3 (connect and listen, with retransmission verified against the host’s own TCP stack), a DHCP client and IPv6 foundations; ACPI table discovery, the I/O APIC cutover that retires the 8259 PIC, and ACPI S5 power-off; a package-signing key hierarchy under an offline root, with scoped certificates and signed revocation; audit anchoring off the disk; guard pages under the kernel’s Ring 3 interrupt (RSP0) and double-fault stacks (not yet the syscall stack); and a fix for a kernel panic any program could trigger with a null pointer and a zero length. UEFI and BIOS boot images built by the release CI run on Linux, published with SHA-256.',
    download: '/download#v0.9.0',
    contents:
      'V0.9 — Transport, Interrupt Cutover & Trust, on top of everything in V0.8: Ring 3 processes in per-process address spaces with preemptive scheduling, capability handles enforced at the syscall boundary, a persistent crash-consistent filesystem on NVMe, a compositor with PS/2 and USB input, AC97 audio, an IPv4 stack (ARP/ICMP/UDP/DNS) with capability-scoped sockets, Ed25519-signed packages with atomic update and rollback, a hash-chained audit trail, SMEP/SMAP/UMIP. New in this release: TCP from Ring 3 (connect and listen, with retransmission), a DHCP client (the console’s dhcp command), IPv6 foundations (link-local and SLAAC addresses, neighbour discovery, ICMPv6 echo), the I/O APIC cutover with the 8259 PIC retired, ACPI S5 power-off (poweroff), packages trusted through an offline-root key hierarchy with scoped certificates and signed revocation, audit anchoring off the disk, and guard pages under the kernel’s Ring 3 interrupt (RSP0) and double-fault stacks (not yet the syscall stack).',
    limitations: [
      'TCP is IPv4 only: one connection per listen, no congestion control beyond an in-flight cap. IPv6 has no UDP, TCP or Ring 3 sockets.',
      'DHCP runs only on demand (dhcp); at boot the address is the static 10.0.2.15/24.',
      'No userspace init; the console is part of the kernel.',
      'Filesystem: ≤ 12 files, no directories, no space reuse.',
      'The boot image itself is not authenticated; no real package is signed by the release key yet.',
      'Found after the release (SEC10-001): any program can panic the kernel by pointing cap_list at its own read-only memory — the kernel checks that a user buffer is mapped, not that the program may write it. Fixed in v0.10.0.',
      'One CPU; software rendering only; no suspend/resume.',
    ],
    archive: {
      builtBy: 'GitHub Actions run 34760629701 (ubuntu-24.04), commit f06673e00f27c3e5ef3df756a452969464d2b558',
      files: [
        {
          file: 'itisyou-os-0.9.0-x86_64-uefi.img',
          firmware: 'UEFI (OVMF)',
          bytes: 4259840,
          sha256: '3d252547926ba497559d7419e2d77803c2af69148cec45ba5a83d6cdddfc443e',
        },
        {
          file: 'itisyou-os-0.9.0-x86_64-bios.img',
          firmware: 'BIOS (SeaBIOS)',
          bytes: 5735424,
          sha256: 'eb3ceb46782b9d04cd40dc61aecce12a8cdbc4ed089b556675f1623c01dd8232',
        },
      ],
      notice:
        'v0.9.0 has a denial of service found after its release and fixed in v0.10.0 (SEC10-001): any program could panic its kernel by pointing cap_list at its own read-only memory.',
    },
  },
  {
    tag: 'v0.8.1',
    commit: 'bf32b53',
    date: '2026-09-13',
    milestone: 'V0.8 — release-integrity closeout',
    ciRun: '34750316003',
    summary:
      'No new kernel function. Corrects v0.8.0, whose kernel reported 0.7.0-dev: the version is now consistent everywhere and a build gate fails on any future drift between the kernel, the status metadata and the requirement matrix. Stale public claims corrected; the site is verified in a real browser. First public download: reproducible, checksummed UEFI and BIOS boot images built by CI and boot-tested byte for byte.',
    download: '/download#v0.8.1',
    contents:
      'V0.8 — Networking, Authenticity & Hardening, as the v0.8.1 closeout: Ring 3 processes in per-process address spaces with preemptive scheduling, capability handles enforced at the syscall boundary, a persistent crash-consistent filesystem on NVMe, a compositor with PS/2 and USB input, AC97 audio, an IPv4 stack (ARP/ICMP/UDP/DNS) with capability-scoped sockets, Ed25519-signed packages with atomic update and rollback, a hash-chained audit trail, SMEP/SMAP/UMIP.',
    limitations: [
      'No TCP, DHCP or IPv6; one static IPv4 address (10.0.2.15/24).',
      'No userspace init; the console is part of the kernel.',
      'Filesystem: ≤ 12 files, no directories, no space reuse.',
      'Package trust root is a published development key.',
      'One CPU; software rendering only; no power management.',
    ],
    archive: {
      builtBy: 'GitHub Actions run 34750316003 (ubuntu-24.04), commit bf32b5337d9503b47bb4cece7d5436a0b1804cca',
      files: [
        {
          file: 'itisyou-os-0.8.1-x86_64-uefi.img',
          firmware: 'UEFI (OVMF)',
          bytes: 4259840,
          sha256: '93750593a7b56d149e1b90e023d111a8aac4fea401fa3a7ce6249b78212cbd88',
        },
        {
          file: 'itisyou-os-0.8.1-x86_64-bios.img',
          firmware: 'BIOS (SeaBIOS)',
          bytes: 4686848,
          sha256: '4a84705bec3c8a2ee0d80cceeb0b5b1562cf237936dc686035ff6f6494f410ea',
        },
      ],
      notice:
        'The UEFI image first uploaded for v0.8.1 (SHA-256 eb7d566c7fcafd9a4ee672b0ede9b3c3456b9577b799698fd982d2ff72c45571) was replaced before the tag, because a clean rebuild did not reproduce it (it carried its loader’s link time). It boots the same system, but it is not the release and cannot be reproduced.',
    },
  },
  {
    tag: 'v0.8.0',
    commit: '84d6b24',
    date: '2026-09-05',
    milestone: 'V0.8 — Networking, Authenticity & Hardening',
    ciRun: '33988391309',
    summary:
      'e1000 + ARP/IPv4/ICMP/UDP/DNS with port-scoped Ring 3 sockets; Ed25519-signed packages; capability handles as the enforcement path; userspace filesystem writes; APIC/MSI-X; xHCI; persistent hash-chained audit; SMEP/SMAP/UMIP. Its kernel misreported its version (0.7.0-dev), corrected in v0.8.1.',
    download: null,
  },
  {
    tag: 'v0.7.0',
    commit: '00b5eea',
    date: '2026-09-03',
    milestone: 'V0.7 — System Platform',
    ciRun: '33812762322',
    summary:
      'Capability model (default deny, no amplification), filesystem sandbox, service supervisor, ITPKG packages with SHA-256 integrity, atomic update/rollback/recovery across reboots, audit trail.',
    download: null,
  },
  {
    tag: 'v0.6.0',
    commit: 'cccf5a7',
    date: '2026-09-02',
    milestone: 'V0.6 — Hardware Expansion',
    ciRun: '33635955858',
    summary: 'Device/driver model, PCI BARs and capabilities, UHCI + USB HID keyboard, AC97 audio output, unified input.',
    download: null,
  },
  {
    tag: 'v0.5.0',
    commit: '5be3c0c',
    date: '2026-09-02',
    milestone: 'V0.5 — Graphics + Input + Desktop',
    ciRun: '33628363046',
    summary: 'Framebuffer graphics, ownership-isolated compositor, GUI syscalls, PS/2 keyboard and mouse, interactive desktop.',
    download: null,
  },
  {
    tag: 'v0.4.0',
    commit: 'b1af235',
    date: '2026-09-02',
    milestone: 'V0.4 — Preemption + Persistent Storage',
    ciRun: '33619289190',
    summary: 'Timer-driven preemption of Ring 3, NVMe write + flush, ITFS with crash-consistent superblocks and reboot persistence.',
    download: null,
  },
  {
    tag: 'v0.3.0',
    commit: 'd83942b',
    date: '2026-09-02',
    milestone: 'V0.3 — Process Isolation + Storage Foundation',
    ciRun: '33609090873',
    summary: 'Per-process address spaces, concurrent processes, spawn/wait, IPC, PCI enumeration, read-only NVMe.',
    download: null,
  },
  {
    tag: 'v0.2.0',
    commit: 'ee8d57c',
    date: '2026-09-02',
    milestone: 'V0.2 — Userspace Foundation',
    ciRun: '33604047455',
    summary: 'Ring 3 ELF64 programs, strict loader with W^X, syscall ABI, crash containment.',
    download: null,
  },
  {
    tag: 'v0.1.0',
    commit: '5a4b008',
    date: '2026-09-02',
    milestone: 'V0.1 — Kernel Foundation',
    ciRun: '33599387686',
    summary: 'Bootable x86_64 kernel: memory management, interrupts, scheduler, VFS/initramfs, shell, QEMU test harness.',
    download: null,
  },
];
