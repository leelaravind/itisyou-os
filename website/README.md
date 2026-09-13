# ITISYOU OS — public engineering website

Static [Astro](https://astro.build) site for **https://os.itisyou.app**, implementing the
Stitch design system preserved in `../design/stitch/` (see `DESIGN.md` there).
Deployed as static assets on Cloudflare by the main agent; this directory only
produces `dist/`.

## Commands

Run from `website/`. Per project storage rules, set the npm cache off `C:` first:

```powershell
$env:npm_config_cache = 'E:\claude-tmp\npm-cache'   # or G:\claude-tmp when attached
npm install          # once
npm run dev          # sync status/docs, then dev server
npm run build        # sync status/docs, then static build -> dist/
npm run preview      # serve the built dist/ locally
npm run verify       # sync + astro check (types/a11y diagnostics) + build; must be clean
```

`predev`/`prebuild` run `npm run sync` automatically:

- `scripts/sync-status.mjs` copies `../status/current.json` → `src/data/status.json`
  (validated against the allowed status vocabulary; build fails on drift).
- `scripts/sync-docs.mjs` copies the seven public repo docs → `src/content/docs/`
  so `/docs/[slug]` renders the actual repository markdown at build time.

## Truthfulness model (binding)

- Every status/version/verification claim on the site comes from
  `src/data/status.json` (synced from `status/current.json`) via `src/lib/status.ts`.
  Pages never hardcode statuses.
- Optional fields (`commit`, `lastVerifiedAt`) may be `null`; the UI renders an
  honest “—” / NOT YET VERIFIED state. The site builds cleanly with them null.
- The Stitch mockups contain fake placeholder data (versions, dates, commit
  hashes, stars, benchmarks). Only the **visual system** was reproduced; all
  content is grounded in `../docs/*.md` and `status/current.json`.
- No download links, test counts, benchmarks, CI badges or hardware claims are
  rendered anywhere until real evidence exists.

## Structure

- `src/layouts/Base.astro` — head/meta/OG, fonts, skip link, header/footer chrome
- `src/components/` — `StatusTag` (single status→style map), `BuildCard`,
  `BootSequence` (B000–B150 from `src/lib/stages.ts`, statuses from status.json),
  `VerificationLoop`, `Header`, `Footer`, `Breadcrumbs`, `WarningBanner`
- `src/lib/` — typed status access, boot stages, roadmap/journal content
  (mirrors `docs/ROADMAP.md` / `docs/DEVELOPMENT_STORY.md`), site nav + docs registry
- `src/pages/` — `/`, `/architecture`, `/build`, `/roadmap`, `/security`,
  `/engineering`, `/docs` + `/docs/[slug]`, `/releases`, `/changelog`,
  `/philosophy`, `/faq`, `/source`, `404`
- `src/styles/global.css` — Tailwind v4 theme tokens from `DESIGN.md` + design
  primitives (`.panel`, `.log-block`, `.status-*`, `.btn`, `.doc-prose`)
- `public/_headers` — Cloudflare security headers (strict CSP, no inline
  scripts/styles; `astro.config.mjs` disables inlined stylesheets and Shiki for this)
- `public/og.svg`, `public/favicon.svg` — generated wordmark assets (no fake imagery)

## Fonts

Self-hosted via `@fontsource-variable/geist` and
`@fontsource-variable/jetbrains-mono` (no Google Fonts CDN at runtime,
`font-display: swap`). The two latin variable woff2 files are preloaded in
`Base.astro`; full fallback stacks are defined in the theme.

## Accessibility

WCAG 2.2 AA targets: semantic landmarks, one `h1` per page, skip link, visible
1px white focus outline, keyboard-navigable no-JS mobile nav and FAQ
(`<details>/<summary>`), ≥44px touch targets in nav, status tags always carry
text (never color alone), diagrams are accessible lists/`figure`s with text
alternatives, wide tables scroll inside their own container,
`prefers-reduced-motion` gates all transitions. Contrast: `#7A8C99` on `#050505`
measures ≈5.8:1, `#8e9192` ≈6.4:1, `#3b82f6` ≈5.5:1 (all ≥4.5:1).

## Design deviations

Deliberate deviations from the Stitch export (with reasons) are recorded in
`DESIGN_DEVIATIONS.md`.
