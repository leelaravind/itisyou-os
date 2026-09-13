# Deployment — os.itisyou.app

**Last release:** V0.8 — `status/current.json` stamped to CI-verified commit
`e6e3496` (run `33987853808`); staging version
`edaab8e1-37e3-4c3a-a13b-398494ab988c`, production version
`a3544b0a-5fe1-44ab-8dfc-b2bd4a3dda0d`.

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
