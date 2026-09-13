# Deployment — os.itisyou.app

**Latest release:** v0.10.0 (2026-09-13) — release commit `c936bc7`, CI run
`34782271250`; first published from the release content at staging
`bed566ad-39ee-4c34-a695-0429cd6e2f0b` / production
`239a2140-e685-4595-bd9f-089c78f4e60b`, then redeployed from the stamp commit
`96f5d68` (so the rendered docs pages carry the stamped text) at staging
`7d140288-eb36-4740-aa84-1239db9960e2` / production
`b56300a5-ceec-4e72-b9d1-3936f4f31b01`, both re-verified; tag `v0.10.0` on
`96f5d68` (CI `34783340401` green); images under `/downloads/v0.10.0/`
(staged from the CI artifact after `release-boot-test.ps1`), v0.9.0 and v0.8.1
kept under their own paths (copied in by hand after `sha256sum -c` against
their live SHA256SUMS). Verified after deploy on both: `downloads.mjs verify`,
a direct digest check of both archives, `browser-verify.mjs` (26 page loads,
0 console errors, 0 failed requests).

**Latest deploy:** 2026-09-13 23:50, disclosure of three audit-trail defects
in v0.10.0 and every release since v0.8.0, found during V0.11 and fixed on
the V0.11 branch — AUDIT11-001 (an untouched trail reads as TAMPERED),
SEC11-001 (programs can rewrite the trail or a package's commit marker),
AUDIT11-002 (forged kernel lines through echoed file names) — and the stale
"(in progress)" heading of the V0.10 requirements — staging
`37c0c1d2-e7e5-4937-b544-69383f93e547`, production
`24574ae3-ff25-4ae7-931a-5313b96c686b`. Verified on both: `downloads.mjs
verify` (v0.10.0 byte-exact), the v0.9.0 and v0.8.1 archives downloaded and
checked against their SHA256SUMS, `browser-verify.mjs` (26 page loads, 0
console errors, 0 failed requests), and the disclosure present on
`/security`, `/download`, `/changelog`, `/engineering` and the
known-limitations, threat-model and security-model docs pages.

**Earlier deploy:** 2026-09-13 20:07, disclosure of SEC10-001 (any program can
panic the v0.9.0 kernel through `cap_list` into its own read-only memory;
fixed on the V0.10 branch) — staging `52ee44d7-cde6-43c3-91d5-cd1d35611329`,
production `d6275adf-7be2-4e68-8404-48fe5dafbbb8`; `downloads.mjs verify`
(v0.9.0 byte-exact), a direct digest check of the v0.8.1 archive and
`browser-verify.mjs` (26 page loads, 0 console errors, 0 failed requests)
on both. The v0.9.0 images were staged from the bytes verified after the
17:20 deploy, the v0.8.1 archive from its release directory after
`sha256sum -c` against the live SHA256SUMS.

**Before that:** 2026-09-13 17:20, commit `ff1af2c` (the V0.9 stack-guard
claim corrected) — production `b2106044…`, browser-verified.

**Previous deploy:** 2026-09-13 16:16, commit `42399b9` (stale /docs pages and
the home layer diagram corrected) — staging `e0080833-7475-47af-ba1a-e628d1cefedf`,
production `169d50f9-28c7-49a5-81a1-be15e5c7d408`; browser-verify and downloads
verify repeated (v0.9.0 byte-exact, v0.8.1 archive byte-exact).

**Earlier still:** 2026-09-13 15:25, commit `8fafda5` (four overstated claims
corrected after adversarial review) — staging `dbc396a6-eca5-4f14-8b9f-68dd8730fe5b`,
production `cf7a7a97-b55a-4b45-885f-74e3e1f00caf`; downloads and browser
verification repeated (v0.9.0 byte-exact, v0.8.1 archive byte-exact).

**Last release:** v0.9.0 (2026-09-13) — release commit `f06673e`, CI run
`34760629701`; staging `896ebfe6-1504-4aeb-9c7b-6f978944584f`, production
`1ba155b6-30e9-4f51-bd2e-5414ea29a968`; images under `/downloads/v0.9.0/`,
v0.8.1 kept under `/downloads/v0.8.1/` (staged from its verified release bytes
next to the v0.9.0 files — `downloads.mjs stage` handles only the current
manifest, so the archive is copied in by hand after a digest check). Both
verified after deploy with `downloads.mjs verify`, a direct digest check of the
archive and `browser-verify.mjs`.

**Last release:** v0.8.1 — `status/current.json` stamped to CI-verified commit
`bf32b53` (run `34750316003`); staging version
`5ad339cd-bd0b-4752-862b-7dcc79a37c0f`, production version
`5436979f-5e4c-42ea-af02-32dbcf5b364a`; release images served from
`/downloads/v0.8.1/`. (V0.8.0: commit `e6e3496`, run `33987853808`, production
`a3544b0a-5fe1-44ab-8dfc-b2bd4a3dda0d`.)

**Last deploy (content only, no release):** 2026-09-13 13:36 BST, commit
`b0c4d05` — the site shows V0.9 as in development with its locally verified
work and the release it is waiting on (CI blocked by GitHub billing); the
download is unchanged. Staging `5499277a-9455-41e4-8613-c6e7b0af93bb`,
production `7d52f221-5abe-46d6-a25c-296f8f087a6c`. Both verified after
deploy: `downloads.mjs verify` (v0.8.1 images byte-exact, SHA-256 and headers)
and `browser-verify.mjs` (26 page loads desktop+phone, 0 console errors, 0
failed requests, headers ok, home shows `v0.8.1` and `V0.9`). The v0.8.1
images were staged from the bytes verified at the v0.8.1 release — every
static-assets deploy must stage them, or it would delete the download.

## Release downloads

Boot images are static assets of the same Worker (each well under the 25 MiB
asset limit), so publishing them needs no new infrastructure or cost:

```powershell
gh run download <ci-run> -n boot-images -D <dir>          # the CI-built bytes
# rename to itisyou-os-<ver>-x86_64-{uefi,bios}.img, then:
scripts\release-boot-test.ps1 -Dir <dir> -Version <ver>    # boot those exact files
# record file/bytes/sha256 in website/src/data/downloads.json, then:
cd website; npm run verify
node scripts/downloads.mjs stage <dir>                      # refuses any mismatch
npx wrangler deploy --env staging
node scripts/downloads.mjs verify https://os-itisyou-app-staging.kpleelaaravind.workers.dev
```

`public/_headers` serves `/downloads/*` as `application/octet-stream`,
`Content-Disposition: attachment`, immutable. A fix is a new version under a new
path, never an overwritten file.

## Topology

- **Model:** Cloudflare Worker with static assets only (no server code).
  The site is fully static HTML/CSS built by Astro (`website/dist`).
- **Config:** `website/wrangler.jsonc` (committed; no dashboard-only state).
  Account: the authenticated ITISYOU Cloudflare account (id
  `a0365f6aaae5fe32b3fdb8fa08fd000c`); auth via the machine's existing
  wrangler OAuth session — no tokens in the repo.
- **Naming convention** (matches sibling ITISYOU services):
  - `os-itisyou-app-staging` → `https://os-itisyou-app-staging.kpleelaaravind.workers.dev`
  - `os-itisyou-app-production` → `https://os.itisyou.app` (custom domain on
    the `itisyou.app` zone, TLS managed by Cloudflare)

## Pipeline

```powershell
cd website
npm run verify                        # sync status/docs, astro check, build
npx wrangler deploy --env staging     # staging (workers.dev)
# browser-verify staging, then:
npx wrangler deploy --env production  # production + custom domain
```

Security headers/CSP ship via `website/public/_headers` (copied into
`dist/`). Status truth: `scripts/sync-status.mjs` re-copies
`status/current.json` on every build and fails the build on vocabulary
drift — the deployed site cannot claim more than the repository evidence.

## Verification after deploy (required — deploy output is not proof)

```powershell
cd website
node scripts/browser-verify.mjs --base https://os-itisyou-app-staging.kpleelaaravind.workers.dev --expect-text "v0.8.1"
node scripts/browser-verify.mjs --base https://os.itisyou.app --expect-text "v0.8.1"
```

The verifier drives a real (headless) Chromium: every route at desktop and
phone widths, console errors, CSP/blocked-resource errors, failed requests,
horizontal overflow, every internal link, the 404 page and the security
headers (`docs/TESTING.md`). Alongside it:

- fetch `https://os.itisyou.app` over TLS and confirm the rendered version and
  commit match `status/current.json`;
- record the deployed Worker version IDs and the verifier result in
  `docs/REQUIREMENTS.md` (the release's WEB rows) and `docs/CURRENT_STATUS.md`.

## Rollback

Static assets deployments are atomic per deploy. Roll back by re-deploying
the previous git state:

```powershell
git checkout <last-good-sha> -- website
cd website; npm run verify; npx wrangler deploy --env production
```

Cloudflare's dashboard "rollback to previous deployment" also works for the
Worker; prefer the git path so the repo stays the source of truth.
