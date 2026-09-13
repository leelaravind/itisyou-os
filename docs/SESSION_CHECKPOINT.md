# Session checkpoint — resumable state

**Timestamp:** 2026-09-13 ~13:20 Europe/London (session 4; final update at wind-up)
**Repository:** `E:\Project\itisyou-os` · branch `main` · remote
https://github.com/leelaravind/itisyou-os (private)
**Tags:** `v0.1.0` … `v0.8.1` (all on green-CI commits). No `v0.9.0` yet.
**Owner deadline for this session:** hard stop **15:44 BST 2026-09-13**
(6 hours from 09:44); wind-up starts 14:44.

## Where things stand

- **Latest release: `v0.8.1`** (release-integrity closeout; content commit
  `bf32b53`, CI `34750316003`). Its CI-built, bit-reproducible images are the
  public download at https://os.itisyou.app/download:
  UEFI `93750593a7b56d149e1b90e023d111a8aac4fea401fa3a7ce6249b78212cbd88`
  (4 259 840 B), BIOS
  `4a84705bec3c8a2ee0d80cceeb0b5b1562cf237936dc686035ff6f6494f410ea`
  (4 686 848 B).
- **V0.9 — Transport, Interrupt Cutover & Trust: every feature row is
  IMPLEMENTED+VERIFIED** (`docs/REQUIREMENTS.md` § V0.9): ACPI discovery; I/O
  APIC cutover with the PIC retired (+ the PIT mode-3 fix); ACPI S5 power-off;
  DHCP (+ the broadcast-UDP checksum fix); audit anchoring off-disk; IPv6
  foundations (client + responder paths); TCP — Ring 3 connect and listen,
  retransmission under injected loss, passive open, all against the host OS's
  own TCP stack (ADR-0020); program-exit release of network resources on every
  exit path (a V0.8 leak, NET09-005); the package-signing key hierarchy
  (ADR-0021: offline root, scoped certificates with release-epoch windows,
  signed revocation; resolves the CI-secret/reproducibility question); guard
  pages on the static kernel stacks (HARD09-001). Only REG-V09 and CI-V09
  (release-level rows) remain non-terminal.
- **V0.9 is NOT released.** GitHub Actions refuses to start any job on this
  account: "The job was not started because recent account payments have
  failed or your spending limit needs to be increased" (first seen on run
  `34755059496`, commit `06a1eca`). Only the owner can clear that (GitHub →
  Settings → Billing & plans). Until then the verification of record is the
  full local gate (`scripts/verify.ps1`) run in an isolated worktree at each
  exact commit — see the gate list below.
- **Root and release-signer private keys** (KEY09-001) were generated with the
  OS CSPRNG into `E:\secrets\itisyou-os\` (`root.seed`,
  `release-signer.seed`) — OUTSIDE the repository, never committed. The owner
  should move them to offline storage. Only public material is in `keys/`.
- Draft PR #1 (`w0-01-freeze-normative-inputs`) is a separate, unmerged,
  owner-gated architecture track. Left untouched.

## Local gates this session (isolated worktree `E:\claude-tmp\wt-v09b`)

| Commit | Result |
|---|---|
| `82d72be` | VERIFY OK, 27/27 QEMU legs |
| `e20b4d5` | VERIFY OK, 31/31 |
| `06a1eca` | VERIFY OK, 34/34, 353 host tests |
| `1e8a909` | VERIFY OK, 35/35, 364 host tests |
| `1a4bf96` | FAILED 35/36 — the shell leg's `panic-test` usage text had been reworded; fixed in `7367b89` |
| `7367b89` | (running at the time of writing — see the final update) |

## Next actions, in order (V0.9 release runbook)

1. Owner restores GitHub Actions billing. Re-run CI on `main`
   (`gh run rerun <id>` or push) and require it green — including the
   `reproducibility` job and the new legs `net-ipv6-bios`,
   `net-ipv6-responder-bios`, `net-tcp-bios`, `trust-bios`, `stack-guard-bios`.
2. Release commit A: workspace `version = "0.9.0"` (Cargo.toml + Cargo.lock),
   `status/current.json` `version` 0.9.0; the shell legs' `itisyou-os 0.9.0-dev`
   markers in `scripts/test.ps1` and `.github/workflows/ci.yml`; roadmap/site
   text; `docs/REQUIREMENTS.md` REL09 rows. `scripts/verify.ps1` → push → CI
   green.
3. `gh run download <run> -n boot-images`, rename to
   `itisyou-os-0.9.0-x86_64-{uefi,bios}.img`,
   `scripts\release-boot-test.ps1 -Dir <dir> -Version 0.9.0`; record bytes and
   SHA-256 in `website/src/data/downloads.json` (move v0.8.1 into the release
   history as the downloads page does), `npm run verify`,
   `node scripts/downloads.mjs stage <dir>`.
4. Deploy staging → `downloads.mjs verify` + `browser-verify.mjs` → production
   → the same verification (procedure: `docs/DEPLOYMENT.md`). IMPORTANT: every
   deploy must stage the release files first — a static-assets deploy without
   `dist/downloads/` would delete the live download.
5. Commit B: stamp `status/current.json` (commit, `verified`,
   `lastVerifiedAt`), WEB/CI rows → push → CI green →
   `node website/scripts/check-consistency.mjs --release` → tag `v0.9.0` → push
   the tag.

After V0.9: V0.10 Userspace System (`docs/ROADMAP.md`).

## Things a resuming session will want to know

- `scripts/test.ps1` is the canonical matrix; `scripts/verify.ps1` wraps it with
  doctor, fmt, both clippy gates, the website build and the secret scan
  (~10 min in a warm worktree).
- CI (`.github/workflows/ci.yml`) duplicates the QEMU legs INLINE rather than
  calling `test.ps1`, so a new leg must be added in BOTH places.
- The shell-test legs assert the kernel's version string and the `panic-test`
  usage line; keep both in step with the kernel.
- A `--require` string containing a double quote does not survive native
  argument quoting on Windows. Assert the quoted part separately.
- The runner's `-cpu qemu64,+smep,+smap,+umip` is load-bearing.
- Large kernel structures must never be built by value on a kernel stack (the
  TCP `Tcb` is 8 KB; see `Tcb::connect_in_place`). The RSP0 and double-fault
  stacks now have guard pages; heap-allocated task stacks do not.
- Keys: `cargo run -q -p keytool -- check "$(cat keys/root.pub.hex)" <file>`;
  issuing certificates or revocation lists needs `E:\secrets\itisyou-os\root.seed`
  (offline only; never in the build or CI).
- Editing repo files from Python: `io.open(..., encoding='utf-8', newline='')`
  and preserve the file's own line endings; the sources are full of em dashes.
- `G:` (external scratch) may be absent; `scripts/env.ps1` falls back to
  `E:\claude-tmp`. Never use C: for scratch.
- Browser verification: `website/scripts/browser-verify.mjs` drives the
  installed Chrome headless (profile under `$ITISYOU_SCRATCH`).

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu (has
`user`/slirp networking with `hostfwd`); Chrome 153 at the default path; never
mix kernel + host packages in one cargo invocation; user programs build static
no-pie.

## Blockers

- **External: GitHub Actions billing** (above). Blocks CI and therefore the
  V0.9 release and tag. Nothing else is blocked.
