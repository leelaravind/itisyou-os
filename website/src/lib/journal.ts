/**
 * Development-story entries — content from docs/DEVELOPMENT_STORY.md
 * (continuously updated engineering log; decisions, failures, root causes,
 * fixes). Only entries that exist in the repo log appear here; nothing is
 * invented. Newest first for display.
 */
export interface JournalEntry {
  date: string;
  time: string;
  session: string;
  title: string;
  what: string;
  detail: string;
  /** Real evidence line (file paths / observed errors), or null when none. */
  evidence: string | null;
  /** Module-style status for the tag (allowed vocabulary). */
  status: 'in-development' | 'implemented' | 'blocked';
}

export const JOURNAL: JournalEntry[] = [
  {
    date: '2026-09-13',
    time: '09:45',
    session: 'Session 4 — audit + V0.8.1',
    title: 'Audit: the V0.8 engineering reproduces; the V0.8 release did not agree with itself',
    what:
      'An independent audit re-ran the whole gate on the v0.8.0 commit before trusting any document: 24/24 QEMU legs, selftest 113/0 on BIOS and UEFI, 292 host tests — the engineering held. The release around it did not: the tagged kernel reported itself as 0.7.0-dev, two requirement rows used a state ("NOT DONE") the plan does not allow, and the live site still said "no network stack exists in V0.1".',
    detail:
      'The version drift had been tested in: the shell-test legs asserted the same stale "0.7.0-dev" string the kernel printed, so the check that should have caught it pinned it instead. The fix is v0.8.1 plus a gate that compares the Cargo version, the status metadata and the requirement states on every build — run against the v0.8.0 tree it names exactly the three defects that shipped. The site is now verified in a real headless Chromium driven over the DevTools protocol rather than over HTTP alone, and the roadmap after V0.8 was re-derived from dependencies: a userspace-system milestone now precedes the AI layer, which depends on it.',
    evidence:
      'check-consistency.mjs on the v0.8.0 tree: version "0.8.0" != "0.7.0-dev"; NET08-003/NET08-004 state "NOT DONE" outside the vocabulary',
    status: 'implemented',
  },
  {
    date: '2026-09-05',
    time: '18:40',
    session: 'Session 3 — V0.8',
    title: 'V0.8: networking, package authenticity, interrupt modernization, hardening',
    what:
      'An e1000 driver with polled RX/TX descriptor rings, ARP, IPv4, ICMP echo (answered as well as sent), UDP with port-scoped Ring 3 sockets, and DNS. Ed25519 signatures for packages, verified against a compiled-in trust root. Capability-scoped userspace filesystem writes with a crash-atomic overwrite. Local APIC, I/O APIC and MSI-X with real interrupt-delivery evidence. xHCI enumeration and HID input. A hash-chained audit trail that survives reboots. SMEP, SMAP and UMIP.',
    detail:
      'Three bugs came out of RUNNING the network stack rather than reading it, and each is a class worth naming. A `match` on a `Mutex` guard kept the lock alive across the arm that re-locks to send an ARP request \u2014 a self-deadlock on a spin lock with interrupts off, which is a dead machine. Bounded waits were built on the timer tick, which does not advance inside a syscall because SFMASK has cleared IF, so a 1-second timeout burned a 400-million-spin backstop instead; the fix was a TSC calibrated against the PIT, plus making the datagram syscalls non-blocking so waiting happens on the caller\u2019s own scheduling slice. And a tight userspace retry loop emitted one ARP broadcast per attempt \u2014 879 in a single run.',
    evidence:
      'artifacts/qemu/net-bios.result.json \u00b7 guest_arp_replies=1 guest_icmp_replies=1 hostile_sent=true replies_to_hostile=0 \u00b7 bad_ip_csum=0 bad_udp_csum=0 bad_icmp_csum=0',
    status: 'implemented',
  },
  {
    date: '2026-09-05',
    time: '18:30',
    session: 'Session 3 — V0.8',
    title: 'The network is verified against an independent implementation, not against itself',
    what:
      'The test harness brings its own Ethernet peer over a QEMU `dgram` netdev: it answers ARP, ICMP echo, a UDP echo service and DNS, and it is the entire network the guest sees \u2014 no slirp, no host resolver, nothing outside the machine.',
    detail:
      'The peer is deliberately a separate byte-level implementation rather than a second use of kernel_core::net. A test where both ends share a checksum routine proves the two agree, not that either is right. It also probes the guest \u2014 an ARP request and a ping \u2014 because a stack that only ever initiates is not a host on a network, and then sends five frames a correct stack must refuse: a corrupt IP checksum, a ping addressed elsewhere but delivered to our MAC, UDP to an unbound port, an 802.1Q tag, and an ARP whose hardware type contradicts its address lengths. The assertion is that the guest counts them as refused AND answers none of them, detected by content rather than by timing.',
    evidence: 'tools/qemu-runner/src/wire.rs \u00b7 rx_malformed=3 rx_unwanted=2 \u00b7 replies_to_hostile=0',
    status: 'implemented',
  },
  {
    date: '2026-09-05',
    time: '18:20',
    session: 'Session 3 — V0.8',
    title: 'Ed25519 written out, with every curve constant derived rather than transcribed',
    what:
      'Package authenticity needed a signature scheme. It is implemented in kernel-core alongside SHA-256 and SHA-512 rather than pulled in as a dependency, and validated against the RFC 8032 test vectors \u2014 public keys, signatures and verification \u2014 not only against itself.',
    detail:
      'The curve constants (d = -121665/121666, sqrt(-1) = 2^((p-1)/4), the base point from y = 4/5) are computed from small integers at use time. A mistyped 32-byte constant produces a working implementation of a DIFFERENT curve: self-consistent, passing every round-trip test, and unable to verify a single real signature. Deriving them removes the possibility. Integrity and authenticity stay separate steps so that a corrupt download, a package from a stranger and a forged signature are three distinguishable refusals rather than one.',
    evidence:
      'RFC 8032 \u00a77.1 vectors pass \u00b7 [L]B = identity \u00b7 signature result=refused reason=unsigned | untrusted_signer | bad_signature',
    status: 'implemented',
  },
  {
    date: '2026-09-05',
    time: '18:10',
    session: 'Session 3 — V0.8',
    title: 'SMAP inverts the default, so the harness had to be told to allow it',
    what:
      'SMEP, SMAP and UMIP are enabled from CPUID and reported from CR4. SMAP makes kernel access to user pages forbidden by default; the three places that legitimately need it bracket their access with a guard whose Drop closes the window on every path.',
    detail:
      'QEMU\u2019s default qemu64 model advertises none of the three, so the hardening would have been enabled into a void and silently done nothing. The runner now requests +smep,+smap,+umip, which means every leg in the matrix runs with supervisor-mode protection on \u2014 a far stronger statement than one leg that enables it, because the other 23 passing is the evidence that the kernel\u2019s own legitimate access to user memory still works. The harness also gained a --forbid assertion: a refusal prints no line of its own, so the only way to state one is that the marker printed on success never appeared.',
    evidence:
      'harden-bios \u00b7 cpu_protection smep=true smap=true umip=true \u00b7 sgdt from Ring 3 is a contained #GP \u00b7 stack guard exactly 16 pages down',
    status: 'implemented',
  },

  {
    date: '2026-09-02',
    time: '02:30',
    session: 'Session 1 — Foundation',
    title: 'Kernel skeleton and test harness authored',
    what:
      'Workspace laid down: kernel/ (lib + interactive and selftest binaries), crates/kernel-core (host-testable stage/marker contract), tools/image-builder (pure-Rust BIOS/UEFI images + SHA-256 manifest), tools/qemu-runner (deterministic boot assertion harness with JSON evidence).',
    detail:
      'First build failed: this nightly\u2019s cargo rejects `cargo-features = ["bindeps"]` in the manifest. Root cause: artifact dependencies must now be enabled via `[unstable] bindeps = true` in .cargo/config.toml. Fixed there; `per-package-target` remains a manifest feature. Rebuild launched.',
    evidence: 'error: unknown Cargo.toml feature `bindeps` — resolved in .cargo/config.toml',
    status: 'in-development',
  },
  {
    date: '2026-09-02',
    time: '02:20',
    session: 'Session 1 — Foundation',
    title: 'Governance intake and discovery',
    what:
      'Read AGENT_OPERATING_RULES.md and the end-to-end implementation plan; both adopted as binding. Requirement traceability matrix created (docs/REQUIREMENTS.md). Stitch design export preserved unchanged in design/ and extracted as the visual source of truth.',
    detail:
      'Environment discovery and storage rules applied: Rust nightly-2026-09-01 + x86_64-unknown-none toolchain installed to E:\\toolchains, QEMU 11.1.0 extracted to E:\\tools\\qemu without elevation, scratch routed to G:\\claude-tmp. GitHub authenticated; repository to be created private.',
    evidence: 'docs/REQUIREMENTS.md · docs/adr/0001..0003 · design/stitch/ (13 screens + DESIGN.md)',
    status: 'in-development',
  },
];
