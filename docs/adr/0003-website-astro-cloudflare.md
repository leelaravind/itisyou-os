# ADR-0003: Website built with Astro, deployed on Cloudflare

**Status:** Accepted · 2026-09-02

## Context

The public site (plan §14) is a content-heavy engineering record: static
pages, documentation, diagrams, and machine-readable status — with hard
requirements for accessibility, performance (minimal client JS), a strict
CSP, and deployment to `os.itisyou.app` on Cloudflare. The visual source of
truth is the preserved Stitch export in `design/stitch/` (dark, technical-
minimalist, Geist + JetBrains Mono, Tailwind-style utility classes).

## Decision

Build the site with **Astro** (static output) + **Tailwind CSS** (tokens
compiled from the Stitch design system — not the Play CDN), self-hosted
fonts, and deploy as static assets on Cloudflare (Workers static assets /
Pages model, whichever discovery shows the account already uses).

## Alternatives

- **Next.js** — heavier runtime and client JS for zero interactive need.
- **Plain hand-rolled HTML** — no component reuse across 13+ routes, worse
  maintainability for the docs surface.
- **SvelteKit/others** — capable, but Astro's zero-JS-by-default and content
  collections map exactly onto this site's shape.

## Consequences

- All pages render as static HTML; JS only where a real behavior needs it.
- Design tokens are extracted once from `design/stitch/DESIGN.md` into the
  Tailwind theme; deviations from the Stitch export are recorded.
- `status/current.json` is copied/derived at build time so the site can
  never display claims that outrun repository evidence (plan §24).
