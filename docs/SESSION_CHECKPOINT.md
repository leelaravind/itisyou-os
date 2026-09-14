# Session checkpoint — resumable state

**Timestamp:** 2026-09-14 Europe/London (session 5, V1.0 build)
**Repository:** `E:\Project\itisyou-os` · remote
https://github.com/leelaravind/itisyou-os (private)
**Tags:** `v0.1.0` … `v0.10.0` (all on green-CI commits).

**V1.0 BUILD — read first.** V1.0 (Experimental Personal OS, criteria in
`docs/V1_ACCEPTANCE.md`) is being built on branch `v1.0/ipc`, pushed to
`origin/v1.0/ipc` and checked out in the worktree `E:\claude-tmp\wt-v1-ipc`
(HEAD `b99024b`). It is built on top of the unreleased V0.11 work, so the
release sequence is **v0.11.0 first, then v1.0.0** — both blocked on the
same external gate: the repository is private and CI runs only on `main`
and PRs, so a release needs the owner to open a short public CI window (or
fix GitHub billing). A feature-branch push is free and does not run CI.

V1.0 acceptance progress (`docs/REQUIREMENTS.md` § V1.0, each with a QEMU
leg and a mutation control unless noted):

- V1-REL-003 soak / leak accounting (found the per-store MMIO frame leak);
- V1-REL-004 Ring 3 syscall fuzzer (`/bin/sysfuzz`, three capability
  configs × 100k calls) — found PROC1-001, `wait(0)` slept forever;
- V1-REL-005 decoder mutation fuzzer (48 targets);
- V1-REL-006 CI parity (`scripts/check-ci-parity.py`, 72 legs);
- V1-SEC-005 unsafe inventory; V1-SEC-006 IPC channel scoping (ADR-0025);
- V1-SEC-002 whole-attack-surface review — 13-agent workflow
  `wf_bb439a1a-29a`, 37 findings (3 high, 15 medium, 19 low); all 3 highs
  and 16 findings fixed with legs and controls, 9 mediums recorded accepted
  in `docs/KNOWN_LIMITATIONS.md`. Full table: `docs/V1_SECURITY_REVIEW.md`.
  Fix IDs SEC1-001..005, NET1-001..004, FS1-001, USB1-001..003.

Last full local gate on `b99024b` (host loaded by another VM): all ~72 QEMU
legs pass except `net-ipv6-responder-bios` and `ai-retry-bios`, which flaked
under load and **pass in isolation** (`run-legs.ps1`, LEGS-OK) — the two
most timing-sensitive legs, on code paths this branch did not change.
Mechanical checks green (ci-parity 72, unsafe-inventory 196 sites, secret
scan 388 files clean). Mutation controls in `E:\claude-tmp\itisyou-audit\v1\`.

The V1.0 release-time criteria (V1-REL-001/002 whole matrix green ×3 + CI,
V1-REP-* reproducibility/tag, V1-SEC-007/BOOT/ENV/DD website+download) can
only be verified by the release run and stay open until the CI window opens.

---

**V0.11 CLOSURE — read first.** V0.11 (AI-Native System Layer, ADR-0024)
is implemented on branch `v0.11/work`, which is pushed and checked out in
the worktree `E:\claude-tmp\wt-v010w`; `main` is an ancestor of it. Every V0.11
row in `docs/REQUIREMENTS.md` is IMPLEMENTED+VERIFIED, each with a negative
control that was rebuilt and run:

- the audit trail: AUDIT11-001, AUDIT11-002, SEC11-001 (disclosed for v0.10.0
  on `main`, `650a16f`);
- the AI layer: MODEL11-001, VIEW11-001, INFER11-001, KB11-001, AGENT11-001,
  PROP11-001, ACT11-001, ACT11-002;
- terminal escapes: OUT11-001;
- the pre-release adversarial review (workflow `wf_47df3cf5-c0a`: six
  reviewers, per-finding verification, nine confirmed): OUT11-002 and
  SEC11-002 (two v0.10.0 console-evidence defects, disclosed on the site),
  AUDIT11-003, and fixes folded into ACT11-002, PROP11-001, AUDIT11-002 and
  OUT11-001.

Full gate on `9ca320c` (before the review): VERIFY OK, 60/60 QEMU legs, 491
host tests. The S11 closure commit (review fixes, 64 legs) has its own gate;
see the Local gates table. The mutation scripts are in
`E:\claude-tmp\itisyou-audit\v011\` (`mutate-s*.ps1`, logs in `nc-logs\`).

NEXT, the v0.11.0 release, following the v0.10.0 runbook:
1. The release commit: version 0.11.0 in `Cargo.toml`/`Cargo.lock`,
   `status/current.json`, and the shell legs' `itisyou-os 0.11.0-dev` marker
   in `scripts/test.ps1` and `.github/workflows/ci.yml`.
2. Merge into `main`, then scan the full history for secrets.
3. Open a short public window for CI.
4. Download the CI images, run `release-boot-test` (it now also requires the
   pinned model and `inferd`), then stamp.
5. Deploy staging and then production, with the downloads staged and the
   v0.10.0, v0.9.0 and v0.8.1 archives kept.
6. Verify in a browser, then tag.
7. Make the repository private again.

After that comes V1.0, with its acceptance criteria written and committed
first (a draft is in `E:\claude-tmp\itisyou-audit\v1\`).

**V0.10 RELEASED (history).** `v0.10.0`: release commit `c936bc7` (CI
`34782271250`, all four jobs green; reproducible images), stamp commit
`96f5d68` (CI `34783340401` green) carrying the tag. Production
`b56300a5-ceec-4e72-b9d1-3936f4f31b01` serves the CI-built images (UEFI
`f98f5cbe…a792`, BIOS `d8416a58…f098`), boot-tested byte for byte; v0.9.0 and
v0.8.1 stay downloadable. The repository was public for the release runs
(history scanned first) and is PRIVATE again after this commit's CI run.
`v0.10/integration` equals `main` at the release. NEXT: V0.11 — AI-Native
System Layer (docs/ROADMAP.md): design first (it adds Ring 3 services under the
existing capability/audit/approval paths), then implement step by step with
negative controls, as V0.10 was. CI again needs the billing fix or another
short public window. Signing keys stay on the owner's offline USB drive.
**Owner deadline for this session:** hard stop **15:44 BST 2026-09-13**
(6 hours from 09:44); wind-up started 14:44 at the latest.

## Where things stand

**FINAL 16:28 — read first.** Repository PRIVATE (since 16:21:49). `main` at
`42399b9` has green CI (`34765042925`, all four jobs; third short public window
16:15–16:21). That commit corrected stale live pages found by the release
review (/docs/security-model, /docs/architecture, /docs/threat-model,
/docs/testing, the home layer diagram); production `169d50f9-28c7-49a5-81a1-be15e5c7d408`
(staging `e0080833-7475-47af-ba1a-e628d1cefedf`) verified in a real browser, v0.9.0
downloads and the v0.8.1 archive byte-exact. This wind-up commit itself is
docs-only and has no CI run (the repository is private and GitHub billing is
still unresolved). NEXT: continue V0.10 on `v0.10/integration` (draft PR #2,
CI green `34764042157`): userspace init + always-on scheduler, Ring 3 shell,
persistent desktop applications; then the V0.10 release (needs CI: billing fix
or another short public window). Signing keys: offline USB drive (see Keys).

**UPDATE 16:05 — second public window (owner's 1-hour timer, deadline 16:43).**
The flaky leg is FIXED and `main` is GREEN: `net-tcp-bios` required an exact
`owner_exit` line that only appears if the probe exits inside the 1 s
TIME-WAIT; it now asserts the property itself (no connection left owned by
the exited program) — commit `0ead17e`, CI `34763748194` all four jobs green.
V0.10: `v0.10/integration` merged `main` and moved to `0.10.0-dev` (`09584ad`);
draft PR #2 (not for merge — V0.10 is not complete) ran CI `34764042157`:
all four jobs green on Linux (program arguments, ITFS reclamation, task stack
guards on top of v0.9.0). Signing keys moved offline (see Keys below). The
repository goes PRIVATE again at the end of this window.

**FINAL 15:37 (superseded by the update above).** The repository is PRIVATE again (15:37:14).
CI on `8fafda5` (`34762496909`) was green. CI on the docs-only commit `4964cc1`
(`34762696403`) FAILED in one step, "QEMU TCP stream against the host OS TCP
stack (BIOS)" (`net-tcp-bios`), after that leg had passed on `f06673e`,
`997dbb4` and `8fafda5` — a flaky leg on the GitHub runner, not a code change.
First job next session: pull that run's log, find which marker was missed
(host client attempt, loss-injection timing or the retransmit window), make the
leg deterministic, and get main green again. The `v0.9.0` tag (`997dbb4`) had
green CI and stands.

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
- **Keys:** root and release-signer private keys (KEY09-001) were MOVED on
  2026-09-13 ~15:50 to the owner's external USB drive (TOSHIBA EXT, then G:),
  folder `ITISYOU-OS-OFFLINE-SIGNING-KEYS\` (copies verified byte-identical,
  originals under `E:\secrets\itisyou-os` deleted). Keep that drive
  disconnected; issuing certificates or revocation lists needs it. Only public
  material is in `keys/`.
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
| `9ca320c` (v0.11/work, OUT11-001) | VERIFY OK, 60/60, 491 host tests |
| S11 closure (v0.11/work; the commit that adds this line) | VERIFY OK, 64/64, 497 host tests |

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
