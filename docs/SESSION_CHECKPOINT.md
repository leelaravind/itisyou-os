# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~08:30 Europe/London (session 1 continued)
**Repository:** `E:\Project\itisyou-os` · branch `main` · tag `v0.1.0` = V0.1 baseline
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.2 Userspace Foundation — Ring 3/syscalls/isolation verified (commit `cc29cda`)

## V0.2 verified additions (evidence: selftest pass=34 fail=0, BIOS+UEFI)

Ring 3 execution of real ELF64 programs (iretq entry, zeroed GPRs), strict
ELF loader (static ET_EXEC only, W^X, user window, overlap policy),
syscall/sysret ABI (write/exit/yield/getpid + ERR_*), MMU kernel/user
separation (#PF USER_MODE proof), privileged-instruction containment
(#GP cs_rpl=3 proof), clean teardown + reload, shell `run` command.
Remaining for the milestone: per-process page tables → concurrent
processes, fork/exec-style creation, IPC foundations.

## V0.1 verified state (baseline, tag v0.1.0)

- **Kernel**: boots QEMU via BIOS and UEFI/OVMF to B150 acceptance; all
  subsystems verified (PMM, paging W^X, heap, GDT/TSS/IDT + exceptions,
  PIC/PIT timer, cooperative scheduler with real context switch,
  VFS/initramfs, 13-command serial shell). Selftests 27/0 both firmwares;
  intentional-panic path verified; 41 host unit tests.
- **Gates**: `scripts/verify.ps1` full run green (doctor, fmt, split
  clippy, tests, QEMU matrix, website, secret scan).
- **Website**: https://os.itisyou.app LIVE and verified (TLS, 11 routes,
  404, strict CSP headers, console-clean browser journey, truthful
  status driven by `status/current.json` stamped with the verified commit).
  Staging: os-itisyou-app-staging.kpleelaaravind.workers.dev.
- **CI**: fmt/clippy/tests/images green on Linux after the cross-target
  feature-unification fix; QEMU matrix status recorded in
  `docs/REQUIREMENTS.md` (CI-001).

## How to resume (any fresh session)

1. Read `CLAUDE.md` → `AGENT_OPERATING_RULES.md` + `docs/IMPLEMENTATION_PLAN.md`.
2. Dot-source `scripts/env.ps1`; run `scripts/doctor.ps1`.
3. `scripts/verify.ps1` must be green before new work.
4. Next milestone: **V0.2 Userspace Foundation** (docs/ROADMAP.md) — ring 3,
   syscall ABI, ELF loading; also V0.1 leftovers: PS/2 keyboard input,
   timer-driven preemption, stack guard pages (docs/KNOWN_LIMITATIONS.md).

## Environment keys (details in docs/BUILD_AND_RUN.md)

nightly-2026-08-01 pin (upstream #579); toolchains on E:; QEMU 11.1.0 at
E:\tools\qemu; TEMP → G:\claude-tmp\tmp; wrangler OAuth on this machine;
never mix kernel + host packages in one cargo invocation (Cargo.toml note).

## Blockers

None.
