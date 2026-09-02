# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~07:45 Europe/London (session 1, final phase)
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.1 Kernel Foundation — hard target complete and verified

## Final verified state

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
