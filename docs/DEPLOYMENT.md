# Deployment — os.itisyou.app

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
