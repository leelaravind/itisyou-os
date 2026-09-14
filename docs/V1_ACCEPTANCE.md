# V1.0 acceptance criteria — ITISYOU OS

**Status:** written and committed BEFORE any V1.0 acceptance run
(`docs/IMPLEMENTATION_PLAN.md` §26: "Only declare when reliability, security,
recovery, hardware support, installation safety, update safety, and
daily-driver acceptance criteria are explicitly defined and passed").
The commit that adds this file is the one the criteria were fixed at; a
criterion may be tightened later, never loosened, and any change is recorded
here with its reason.

**What V1.0 is.** "Experimental Personal OS", VM-verified (roadmap amendment
2026-09-13): a reproducible, checksummed, downloadable boot image for virtual
machines, with the tested environments stated exactly and physical hardware
explicitly unsupported. V1.0 is not a daily driver and does not claim to be.

**Rule.** V1.0 is declared (tag `v1.0.0`) only when every criterion below is
PASSED with the evidence named in its row. One failed or unmeasured
criterion means no V1.0: the release waits, or the criterion's failure is
fixed — never reworded.

Each criterion becomes a row in `docs/REQUIREMENTS.md` § V1.0 with the same
ID; the result table at the end of this file records the outcome, the
commit and the evidence.

## Reliability

| ID | Criterion | Pass threshold | Evidence |
|---|---|---|---|
| V1-REL-001 | The whole QEMU matrix passes on the release commit, locally and in CI | every leg green in `scripts/verify.ps1` (Windows 11, QEMU 11.1.0) AND in the release commit's CI run (ubuntu-24.04, QEMU 8.2.2); no leg re-run to get there | gate log; CI run id |
| V1-REL-002 | The matrix is repeatable | three consecutive full local runs of the QEMU matrix on the release-candidate commit, zero failures, plus the CI run | three gate logs |
| V1-REL-003 | Long use does not leak | a new soak leg runs a mixed workload — at least 200 process lifecycles, file writes and deletes, IPC round trips, agent diagnoses, network pings — and the kernel's own accounting is identical between a checkpoint after warm-up and the end: free physical frames, heap bytes in use, process-table slots, IPC queued messages, sockets, output line buffers, capability handles | soak leg + its control (a deliberate leak must fail it) |
| V1-REL-004 | No Ring 3 syscall sequence crashes or hangs the kernel | a seeded Ring 3 fuzzer issues at least 100 000 syscalls with pseudo-random numbers and arguments in each of: no capabilities; the console's default set inside a sandbox; the agent's set. The kernel never panics or hangs, and a sanity pass afterwards (`ps`, `svc`, a program run, `audit verify`) is clean | fuzz leg per configuration, seed recorded |
| V1-REL-005 | No decoder of untrusted bytes panics | every `kernel_core` decoder that parses bytes a program, a disk, a package or the network supplies survives at least 1 000 000 seeded mutated inputs (valid seeds mutated by bit flips, truncation, splicing, length-field edits) with no panic, no overflow in debug builds and no hang | host fuzz harness; per-decoder counts |
| V1-REL-006 | CI checks what the local matrix checks | a mechanical comparison finds every CI leg's sends, requires, require-orders and forbids equal to `scripts/test.ps1`'s (timeouts may differ) | comparison script output |

## Security

| ID | Criterion | Pass threshold | Evidence |
|---|---|---|---|
| V1-SEC-001 | Every requirement is in a terminal state | `docs/REQUIREMENTS.md` has no non-terminal row (`check-consistency.mjs`), and no BLOCKED row is security-relevant | consistency check |
| V1-SEC-002 | The whole attack surface survives an adversarial review | a multi-agent review of the whole kernel attack surface — every syscall, the ELF loader, VFS and ITFS, packages and the trust hierarchy, the network stack, input and device parsing, ACPI, the AI layer, the audit trail — with every finding verified by an independent refuter. Zero confirmed high-severity findings open at release; every confirmed medium fixed, or recorded in `docs/KNOWN_LIMITATIONS.md` with the reason it is accepted | review run id; findings table |
| V1-SEC-003 | Hardware protections hold on every leg | SMEP, SMAP, UMIP, W^X, NX and every kernel stack guard verified by their legs | harden, stack-guard legs |
| V1-SEC-004 | No secret in the repository, keys offline | full-history secret scan clean; no private key material in any commit; release signing keys only on the offline drive | scan output |
| V1-SEC-005 | Every `unsafe` is inventoried | a script counts every `unsafe` block, fn and impl in `kernel/` and matches them to `docs/UNSAFE_INVENTORY.md`; `kernel-core` forbids `unsafe` | script output |
| V1-SEC-006 | IPC authority is scoped | an IPC capability names the channels it reaches; the agent and inferd reach only the inference channels, tickd only its own; a process cannot read or answer on a channel it was not given | leg + control |
| V1-SEC-007 | What is public about security is current | `docs/SECURITY_MODEL.md`, `docs/THREAT_MODEL.md` and the site's security page describe V1.0, and every defect found in a released version is disclosed for it | page review |

## Recovery

| ID | Criterion | Evidence (existing legs unless noted) |
|---|---|---|
| V1-RCV-001 | Power loss during an update leaves a consistent, bootable system | `update-interrupt`, `update-recovery` |
| V1-RCV-002 | The filesystem survives a crash at any commit point | selftest torn-superblock cases; `fs-persist-*`, `fs-write-persist`, `fs-reclaim-persist` |
| V1-RCV-003 | A dead init and failing services are recovered, bounded | `init-bios`, `services-bg-bios` |
| V1-RCV-004 | A faulting program never takes the kernel down | `harden-bios`, selftest containment checks |
| V1-RCV-005 | A kernel panic is reported and ends the run deterministically | `panic-test-bios` |
| V1-RCV-006 | Tampering with the audit trail is detected and stays reported | `audit-persist-*`, `audit-tamper-*`, `audit-anchor-*` |
| V1-RCV-007 | Recovery is documented | `docs/RECOVERY.md` covers each of the above |

## Update safety

| ID | Criterion | Evidence |
|---|---|---|
| V1-UPD-001 | Only a package signed by a certified, unrevoked key in scope installs | `trust-bios`, `platform-bios` |
| V1-UPD-002 | Updates are atomic and can be rolled back | `platform-bios` |
| V1-UPD-003 | An interrupted update never activates | `update-interrupt`, `update-recovery` |
| V1-UPD-004 | No program can alter the package store or the audit trail | `fs-write-bios` (SEC11-001) |

## Installation and boot safety

| ID | Criterion | Evidence |
|---|---|---|
| V1-BOOT-001 | There is no installer and the image writes no disk it was not explicitly given | docs + the harness's disk configuration |
| V1-BOOT-002 | The published images boot, BIOS and UEFI, from the bytes downloaded from production | `release-boot-test.ps1` on those bytes |
| V1-BOOT-003 | Booting does not change the image files | `release-boot-test.ps1` digests before and after |
| V1-BOOT-004 | The download page warns that physical hardware is unsupported and says how to run the image | page review |

## Supported environment and hardware support

| ID | Criterion | Evidence |
|---|---|---|
| V1-ENV-001 | The tested environments are stated exactly — x86_64 QEMU, `-cpu qemu64,+smep,+smap,+umip -m 256M`, SeaBIOS and OVMF; QEMU 11.1.0 on Windows 11 and QEMU 8.2.2 on Ubuntu 24.04 — and nothing else is claimed | `/download`, release notes |
| V1-HW-001 | Physical hardware is unsupported and untested (the plan's §27 gate is not passed); the emulated devices it drives are listed | `/download`, `docs/KNOWN_LIMITATIONS.md` |

## Daily-driver acceptance

| ID | Criterion | Evidence |
|---|---|---|
| V1-DD-001 | V1.0 claims NO daily-driver suitability: the site and the release notes say it is an experimental VM-only system, and daily-driver research is after V1.0 | page review |

## Reproducible release artifact

| ID | Criterion | Evidence |
|---|---|---|
| V1-REP-001 | Two clean builds produce bit-identical images | CI `reproducibility` job |
| V1-REP-002 | The published files are the CI-built bytes, with SHA-256 and `SHA256SUMS.txt` | `downloads.mjs verify` on staging and production |
| V1-REP-003 | The format is the technically correct one: raw GPT/MBR disk images (`.img`) for UEFI and BIOS, as the bootloader produces, bootable in QEMU as documented | `docs/BUILD_AND_RUN.md`, `/download` |
| V1-REP-004 | Tag `v1.0.0` sits on a commit with green CI | `gh run list` |

## Documentation

| ID | Criterion | Evidence |
|---|---|---|
| V1-DOC-001 | `docs/BUILD_AND_RUN.md` reproduces the images from a clean checkout on Linux (CI) and Windows (local) | CI build job; local gate |
| V1-DOC-002 | The release notes and `docs/KNOWN_LIMITATIONS.md` state every known limit | page review |

## Results

(Filled in by the V1.0 acceptance run: criterion, PASSED/FAILED, commit, evidence.)
