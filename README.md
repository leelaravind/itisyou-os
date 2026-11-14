# ITISYOU OS

**Experimental. Not a product. Virtual machines only — do not install on real hardware.**

ITISYOU OS is a long-term research and engineering project exploring what a
personal operating system could look like if designed today around explicit
privilege boundaries, strong kernel/user isolation, recoverable and auditable
system changes, reproducible builds, and local-first intelligence — where AI
can reason about the system but never holds unrestricted kernel authority.

It is an independently bootable x86_64 operating system written in Rust
(`no_std`), developed and verified exclusively inside QEMU. It is not a Linux
distribution and contains no Linux kernel code at runtime.

The latest release is **V1.0 — Experimental Personal OS** (`v1.0.0`,
VM-verified): Ring 3 processes in per-process address spaces with a
userspace `init` as pid 1, a Ring 3 shell and always-on scheduling;
capability handles enforced at the syscall boundary; a persistent
crash-consistent filesystem on NVMe; a compositor and desktop with PS/2 and
USB (UHCI and xHCI) input and Ring 3 applications; AC97 audio; an IPv4/IPv6
network stack (ARP, ICMP, UDP, DNS, DHCP, TCP) with capability-scoped
sockets; Ed25519-signed packages under an offline-root key hierarchy, with
atomic update and rollback; a hash-chained persistent audit trail; a
diagnostic agent in Ring 3 that can only *propose* actions the console must
approve (intelligence, not authority); and SMEP/SMAP/UMIP hardening. V1.0
was accepted against criteria written down beforehand
([`docs/V1_ACCEPTANCE.md`](docs/V1_ACCEPTANCE.md)); the three rows that need
a GitHub-hosted CI run are recorded BLOCKED, not passed. What is *not*
there — SMP, physical hardware, Wi-Fi, a language model — is listed in
[`docs/KNOWN_LIMITATIONS.md`](docs/KNOWN_LIMITATIONS.md) and sequenced in
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## Current reality

See [`status/current.json`](status/current.json) and
[`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md) for the evidence-backed state
of every subsystem. Anything not marked **Verified** there should be assumed
incomplete. The public engineering site is at
[os.itisyou.app](https://os.itisyou.app).

## Build and run (Windows host)

Prerequisites and exact steps: [`docs/BUILD_AND_RUN.md`](docs/BUILD_AND_RUN.md).

```powershell
# environment doctor (checks toolchain, QEMU, storage routing)
scripts\doctor.ps1

# build kernel + bootable BIOS/UEFI images
scripts\build.ps1

# boot interactively in QEMU (UEFI via OVMF)
scripts\run-qemu.ps1

# full verification gate (fmt, clippy, unit tests, QEMU boot matrix, secret scan)
scripts\verify.ps1
```

## Safety boundary

All kernel execution happens inside QEMU with project-generated disposable
disk images. Nothing in this repository modifies the host's bootloader, EFI
system partition, partitions, Secure Boot state, or firmware — and nothing
here should ever be pointed at a physical disk.

## Repository layout

- `kernel/` — the x86_64 kernel (library + interactive/selftest binaries)
- `crates/kernel-core` — pure host-testable kernel logic (parsers, protocol
  codecs, crypto, filesystem format, capability table)
- `user/` — Ring 3 programs and the `ulib` syscall library
- `tools/image-builder` — builds bootable disk images (pure Rust)
- `tools/qemu-runner` — deterministic QEMU test harness with JSON evidence
- `scripts/` — doctor / build / run / test / verify / secret-scan gates
- `design/` — preserved Stitch design export (visual source of truth)
- `website/` — the public engineering site for os.itisyou.app
- `docs/` — architecture, requirements, security model, development story
- `status/` — machine-readable, evidence-backed project status

## Governance

Development follows [`AGENT_OPERATING_RULES.md`](AGENT_OPERATING_RULES.md)
and [`docs/IMPLEMENTATION_PLAN.md`](docs/IMPLEMENTATION_PLAN.md). Every
requirement ends in exactly one state: implemented + verified, blocked, or
not applicable — with evidence.
