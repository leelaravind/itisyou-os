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

- fetch `https://os.itisyou.app` (status 200, expected content);
- check TLS, headers (CSP, nosniff, frame-ancestors), 404 route;
- browser-test major routes, mobile viewport, console errors;
- record results in `docs/REQUIREMENTS.md` (CF-001).

## Rollback

Static assets deployments are atomic per deploy. Roll back by re-deploying
the previous git state:

```powershell
git checkout <last-good-sha> -- website
cd website; npm run verify; npx wrangler deploy --env production
```

Cloudflare's dashboard "rollback to previous deployment" also works for the
Worker; prefer the git path so the repo stays the source of truth.
