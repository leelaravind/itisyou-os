# Session checkpoint — resumable state

**Timestamp:** 2026-09-13 ~10:30 Europe/London (session 4)
**Repository:** `E:\Project\itisyou-os` · branch `main` · remote
https://github.com/leelaravind/itisyou-os (private)
**Tags:** `v0.1.0` … `v0.8.0` (all on green-CI commits); `v0.8.1` in progress
**Owner deadline for this session:** hard stop **15:44 BST 2026-09-13**
(6 hours from 09:44); wind-up starts 14:44. Whatever is not finished by then is
left committed, pushed and described here.

## Where things stand

- **V0.8 is released** (`v0.8.0` → `84d6b24`, CI `33988391309` green, site
  deployed). The 2026-09-13 audit re-ran the full gate on that commit:
  `VERIFY: OK`, 24/24 QEMU legs, selftest 113/0 (BIOS and UEFI), 292 host
  tests, secret scan clean. The engineering reproduces.
- **V0.8.1 (release-integrity closeout) is released.** It fixed the `v0.8.0`
  version drift, the invalid requirement states and the stale public claims;
  added `website/scripts/check-consistency.mjs` (version + requirement-state
  gate, `--release` before tags), `website/scripts/browser-verify.mjs`
  (headless Chromium over CDP) and the first public download: CI-built,
  bit-reproducible (GPT + embedded-PE normalization, CI job `reproducibility`),
  boot-tested byte for byte (`scripts/release-boot-test.ps1`) and served from
  https://os.itisyou.app/download (`website/scripts/downloads.mjs`). All rows in
  `docs/REQUIREMENTS.md` § V0.8.1 are terminal. Content commit `bf32b53`
  (CI `34750316003`); production `5436979f-5e4c-42ea-af02-32dbcf5b364a`.
- **V0.9 — Transport, Interrupt Cutover & Trust is in progress** (version
  `0.9.0-dev`, status `in-development`; production intentionally stays at the
  `v0.8.1` release until V0.9 closes). Verified and committed: ACPI discovery,
  I/O APIC cutover with the PIC retired (plus the PIT mode-3 double-delivery
  fix), ACPI S5 power-off, DHCP client, broadcast-UDP checksum fix
  (checkpoint `82d72be`, local gate 27/27). Verified, committing next: audit
  anchoring against an off-disk witness (four-boot test). In flight: TCP
  (host core + kernel integration + host-stack echo test) and IPv6 foundations
  (host codec). Not started: KEY09-001 signing-key hierarchy — open design
  question recorded in `docs/REQUIREMENTS.md`: fixture packages baked into the
  image are signed at build time, so an off-tree release key needs a CI secret
  AND breaks third-party reproducibility of the image unless fixtures are signed
  by a separately certified test key. Roadmap: `docs/ROADMAP.md` (2026-09-13
  amendment: V0.10 Userspace System, V0.11 AI layer, daily-driver hardware after
  V1.0).
- Draft PR #1 (`w0-01-freeze-normative-inputs`) is a separate, unmerged,
  owner-gated architecture track. It was deliberately left untouched.

## Release procedure (what "closed" means here)

1. `scripts/verify.ps1` → `VERIFY: OK` (includes the consistency gate).
2. Commit A (implementation + docs; `status/current.json` verification
   `in-development`, commit null) → push → CI green.
3. Commit B: stamp `status/current.json` (commit = A, `verified`,
   `lastVerifiedAt`), CI row → push.
4. `cd website; npm run verify; npx wrangler deploy --env staging` →
   `node scripts/browser-verify.mjs --base <staging> --expect-text vX.Y.Z` →
   `npx wrangler deploy --env production` → browser-verify production.
5. Commit C: WEB rows + deploy IDs → push → CI green →
   `node website/scripts/check-consistency.mjs --release` → tag on C → push tag.

## Things a resuming session will want to know

- `scripts/test.ps1` is the canonical matrix; `scripts/verify.ps1` wraps it with
  doctor, fmt, both clippy gates, the website build and the secret scan (~12 min).
- CI (`.github/workflows/ci.yml`) duplicates the QEMU legs INLINE rather than
  calling `test.ps1`, so a new leg must be added in BOTH places.
- The shell-test legs assert the kernel's version string; bump it with the
  Cargo workspace version and `status/current.json` (the gate enforces the
  latter pair).
- A `--require` string containing a double quote does not survive native
  argument quoting on Windows. Assert the quoted part separately.
- The runner's `-cpu qemu64,+smep,+smap,+umip` is load-bearing.
- Editing repo files from Python: `io.open(..., encoding='utf-8', newline='')`
  and preserve the file's own line endings; the sources are full of em dashes.
- `G:` (external scratch) may be absent; `scripts/env.ps1` falls back to
  `E:\claude-tmp`. Never use C: for scratch.
- Browser verification: the Claude-in-Chrome extension was not connected in
  sessions 3 and 4; `website/scripts/browser-verify.mjs` drives the installed
  Chrome headless instead (profile under `$ITISYOU_SCRATCH`).

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu (has
`user`/slirp networking); Chrome 153 at the default path; never mix kernel +
host packages in one cargo invocation; user programs build static no-pie.

## Blockers

None external. The only constraint is the owner's session deadline above.
