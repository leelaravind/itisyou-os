# Session checkpoint — resumable state

**Timestamp:** 2026-09-13 14:37 Europe/London (session 4, wind-up)
**Repository:** `E:\Project\itisyou-os` · branch `main` · remote
https://github.com/leelaravind/itisyou-os (private)
**Tags:** `v0.1.0` … `v0.9.0` (all on green-CI commits).
**Owner deadline for this session:** hard stop **15:44 BST 2026-09-13**
(6 hours from 09:44); wind-up started 14:44 at the latest.

## Where things stand

**UPDATE 15:10 — v0.9.0 RELEASED.** The owner authorised making the repository
public so GitHub-hosted CI could run (history scanned for secrets first), then
private again. Release commit `f06673e`, CI run `34760629701` green (all four
jobs; reproducibility job matched the published digests); CI-built images
boot-tested byte for byte; production `1ba155b6` serves them at
https://os.itisyou.app/download — UEFI
`3d252547926ba497559d7419e2d77803c2af69148cec45ba5a83d6cdddfc443e`, BIOS
`eb3ceb46782b9d04cd40dc61aecce12a8cdbc4ed089b556675f1623c01dd8232`; v0.8.1 kept.
Tag `v0.9.0` sits on the status-stamp commit after its own green CI. Steps 1–6
of the runbook below are therefore DONE; the next action is step 7 (merge
`v0.10/integration`, continue V0.10). The repository is PRIVATE again, so CI
needs the billing fix (or the same public window) before the next release.

Tag `v0.9.0` → `997dbb4` (CI `34761869462` green), pushed 15:21. A follow-up
commit `8fafda5` corrected four overstated site claims found by the release
workflow's adversarial reviewers; production is now `cf7a7a97`. Reviewer
findings NOT fixed (all accuracy/polish, none affects the download or the
evidence): the /docs pages rendered from ARCHITECTURE.md (interrupt section
still PIC-era), SECURITY_MODEL.md (boundary 7 "no network stack yet") and
THREAT_MODEL.md (V0.8 non-threat heading) need V0.9 wording; TESTING.md says
"24-leg" matrix; the home page's layer diagram and "What we're building now"
heading; changelog cards show the current build on every entry;
`downloads.mjs` stages only the current manifest (the v0.8.1 archive is copied
by hand); releases.astro hard-codes the pre-release file naming; the SEC09-001
title could be narrower ("a zero-length Ring 3 buffer"); V0.10 could be shown
as in development (branch-only) rather than planned. The full list is in the
workflow result of this session (run `wf_471678c2-9e3`).

The rest of this section is the state as of the 14:20 wind-up.

- **Latest release: `v0.8.1`** (content commit `bf32b53`, CI `34750316003`).
  Its CI-built, bit-reproducible images are the public download at
  https://os.itisyou.app/download — UEFI
  `93750593a7b56d149e1b90e023d111a8aac4fea401fa3a7ce6249b78212cbd88`
  (4 259 840 B), BIOS
  `4a84705bec3c8a2ee0d80cceeb0b5b1562cf237936dc686035ff6f6494f410ea`
  (4 686 848 B); re-verified byte-exact on staging and production today.
- **`main` = the V0.9 release candidate.** Every V0.9 feature row in
  `docs/REQUIREMENTS.md` is IMPLEMENTED+VERIFIED: ACPI discovery; I/O APIC
  cutover with the PIC retired; ACPI S5 power-off; DHCP; audit anchoring
  off-disk; IPv6 foundations; TCP — Ring 3 connect and listen, retransmission
  under injected loss, passive open, against the host OS's own TCP stack
  (ADR-0020); network release on every exit path (NET09-005); the signing-key
  hierarchy (ADR-0021); guard pages on the static kernel stacks (HARD09-001);
  zero-length user copies never touching the pointer (SEC09-001 — a V0.9
  kernel-panic DoS through `cap_list(NULL, 0)`, found by the V0.10 work and
  fixed on `main`); the website showing all of this (WEB09-001). Only the
  release-level rows REG-V09 and CI-V09 remain non-terminal.
- **Final local gate on `main` `08d836c`: VERIFY OK, 36/36 QEMU legs, 366
  host tests, secret scan clean** (isolated worktree at that exact commit).
- **V0.9 is NOT released — blocked externally.** GitHub Actions refuses to
  start any job: "The job was not started because recent account payments
  have failed or your spending limit needs to be increased" (every push since
  `06a1eca`, latest run `34757645006`). Only the owner can clear it (GitHub →
  Settings → Billing & plans).
- **Website:** deployed 13:36 from `b0c4d05` — staging
  `5499277a-9455-41e4-8613-c6e7b0af93bb`, production
  `7d52f221-5abe-46d6-a25c-296f8f087a6c` — V0.9 shown as in development
  with its evidence and the pending release; download unchanged (v0.8.1).
  Verified after deploy in a real browser (26 page loads, 0 console errors,
  0 failed requests, headers ok) and by downloading both images. Later
  commits on `main` changed docs and one kernel fix only; the deployed site
  has no stale claim about them (SEC09-001 is not yet on the site; add it at
  the V0.9 release).
- **V0.10 started on branch `v0.10/integration`** (pushed; NOT merged into
  `main`, so `main` stays the V0.9 release candidate): program arguments
  (PROC10-001, syscalls 35 `args` / 36 `spawn_args`, `run … -- args`) and
  ITFS space reclamation (FS10-001, gap allocator, no format change), built by
  two agents in isolated worktrees and merged with `main`. Gate on the first
  integration commit `4ceaa4c`: VERIFY OK, 39/39 legs, 385 host tests. The
  branch tip `b5cc23d` (adds `main`'s later commits and the agent's zero-length
  fix): VERIFY OK, 39/39 legs, 385 host tests. Then guard pages for kernel
  TASK stacks (HARD10-001, `e12329b`, the branch tip): VERIFY OK, 40/40 legs.
- **Keys:** root and release-signer private keys (KEY09-001) are in
  `E:\secrets\itisyou-os\` (`root.seed`, `release-signer.seed`), generated
  with the OS CSPRNG, OUTSIDE the repository, never committed. The owner
  should move them to offline storage. Only public material is in `keys/`.
- Draft PR #1 (`w0-01-freeze-normative-inputs`) is a separate, unmerged,
  owner-gated track. Left untouched.

## Local gates this session (isolated worktrees)

| Commit | Result |
|---|---|
| `82d72be` | VERIFY OK, 27/27 QEMU legs |
| `e20b4d5` | VERIFY OK, 31/31 |
| `06a1eca` | VERIFY OK, 34/34, 353 host tests |
| `1e8a909` | VERIFY OK, 35/35, 364 host tests |
| `1a4bf96` | FAILED 35/36 — a reworded `panic-test` marker; fixed in `7367b89` |
| `7367b89` | VERIFY OK, 36/36, 366 host tests |
| `08d836c` (main, final) | VERIFY OK, 36/36, 366 host tests |
| `4ceaa4c` (v0.10/integration) | VERIFY OK, 39/39, 385 host tests |
| `b5cc23d` (v0.10/integration) | VERIFY OK, 39/39, 385 host tests |
| `e12329b` (v0.10/integration tip) | VERIFY OK, 40/40, 385 host tests |

## Next actions, in order

1. **Owner:** restore GitHub Actions billing.
2. Re-run CI on `main` and require it green, including the `reproducibility`
   job and the V0.9 legs (`net-ipv6-bios`, `net-ipv6-responder-bios`,
   `net-tcp-bios`, `trust-bios`, `stack-guard-bios`).
3. V0.9 release commit A: workspace `version = "0.9.0"` (Cargo.toml +
   Cargo.lock), `status/current.json` version 0.9.0, the shell legs'
   `itisyou-os 0.9.0-dev` markers in `scripts/test.ps1` and
   `.github/workflows/ci.yml`, site text (roadmap `release` field → remove,
   V0.9 → verified), REL09 rows. `scripts/verify.ps1` → push → CI green.
4. `gh run download <run> -n boot-images`; rename to
   `itisyou-os-0.9.0-x86_64-{uefi,bios}.img`;
   `scripts\release-boot-test.ps1 -Dir <dir> -Version 0.9.0`; record bytes and
   SHA-256 in `website/src/data/downloads.json`; `npm run verify`;
   `node scripts/downloads.mjs stage <dir>`.
5. Deploy staging → `downloads.mjs verify` + `browser-verify.mjs` →
   production → the same (`docs/DEPLOYMENT.md`). EVERY static-assets deploy
   must stage the release images first, or it deletes the live download.
6. Commit B: stamp `status/current.json` (commit, `verified`,
   `lastVerifiedAt`) and the WEB/CI rows → push → CI green →
   `node website/scripts/check-consistency.mjs --release` → tag `v0.9.0` →
   push the tag.
7. Merge `v0.10/integration` into `main` (re-run the gate) and continue V0.10:
   userspace init + always-on scheduler, Ring 3 shell, persistent desktop
   applications (`docs/ROADMAP.md`).

## Things a resuming session will want to know

- `scripts/test.ps1` is the canonical matrix; `scripts/verify.ps1` wraps it
  (~8–10 min in a warm worktree). CI duplicates every leg INLINE — add new legs
  in BOTH places.
- QEMU legs share scratch disk-image names under `E:\claude-tmp`
  (`scripts/env.ps1`); never run two gates at once.
- The shell legs assert the kernel version string and the `panic-test` usage
  line.
- `--require` strings must not contain double quotes (Windows native arg
  quoting).
- Never build large kernel structures by value on a kernel stack (the TCP
  `Tcb` is 8 KB; see `Tcb::connect_in_place`/`listen_in_place`).
- Keys: `cargo run -q -p keytool -- check "$(cat keys/root.pub.hex)" <file>`;
  issuing certificates/revocation lists needs the off-tree root seed.
- npm: run `npx wrangler` with `NPM_CONFIG_CACHE=E:\claude-tmp\npm-cache` (the
  default cache points at the absent `G:`).
- Editing repo files from Python: `io.open(..., encoding='utf-8', newline='')`,
  preserve line endings; avoid backslash escapes in heredocs (write a script
  file instead).

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu
(`user`/slirp with `hostfwd`); Chrome 153 at the default path; never mix
kernel + host packages in one cargo invocation; user programs build static
no-pie.

## Blockers

- **External: GitHub Actions billing.** Blocks CI, and therefore the V0.9
  release and tag. Nothing else is blocked.
