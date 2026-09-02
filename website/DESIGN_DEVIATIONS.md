# Design deviations — website vs. Stitch export

The Stitch export in `../design/stitch/` is the canonical visual reference.
The deviations below are deliberate, with reasons. Everything else follows the
export: palette, Geist/JetBrains Mono pairing, sharp 0px corners, 1px
structural lines instead of shadows, 4px body grid pattern, cell-based layout,
mono uppercase tags/breadcrumbs/buttons, status-tag semantics.

## Content deviations (required by governance — truthfulness)

1. **All placeholder data replaced with real data or honest absence.** The
   mockups show `v0.4.2-alpha`, `BUILD_STABLE`, fake dates/commits/SHAs, star
   and fork counts, `CI PASSING`, build times, download artifacts,
   `Multiboot2`, microkernel claims, RISC-V/ARMv9 support, and Verified tags
   on unbuilt subsystems. The site renders version/maturity/milestone/module
   statuses exclusively from `status/current.json`, renders `—` / NOT YET
   VERIFIED where evidence is absent, and describes the boot path truthfully
   (rust-osdev `bootloader` crate, BIOS + UEFI/OVMF, QEMU-only).
2. **Releases page is an honest empty state** instead of the mockup's three
   downloadable releases: no releases exist, so the page explains the planned
   artifact + SHA-256 manifest + evidence model without download buttons.
3. **Source page shows a PRIVATE repository state** instead of the mockup's
   public repo cells with stars/forks/commit feeds — the repository is private
   during early development; no fake GitHub links or statistics are rendered.
4. **404 page log block is website-truthful** ("no route matched") rather than
   the mockup's fictional kernel stack trace, which could read as real kernel
   output. The FAULT visual language is kept.
5. **Terminal/build-log panels render the marker grammar as a labeled
   specification**, not a fake captured boot log (engineering page). No
   invented `[ OK ]` boot lines.

## Visual/technical deviations

6. **Material Symbols icon font removed.** The mockups load Google's icon font
   from a CDN; the site's CSP and performance budget forbid third-party
   runtime fonts. Small unicode glyphs (▲ → + −) and text labels replace the
   few decorative icons. Reason: CSP `font-src 'self'`, no-CDN rule, minimal
   payload.
7. **Google Fonts CDN replaced by self-hosted @fontsource variable fonts**
   (Geist, JetBrains Mono) with preloads and full fallback stacks. Reason:
   no third-party requests, deterministic performance.
8. **No syntax-highlighting colors in code blocks.** Shiki inline styles
   violate the `style-src 'self'` CSP; code/log blocks use the design's mono
   single-color treatment on `#080808` instead. Reason: strict CSP without
   `unsafe-inline`.
9. **Sidebar-navigation screens (architecture, docs, engineering, releases)
   use the shared fixed top header instead of the mockups' left OS-style
   sidebars.** The mockups' sidebars navigate to sections that do not exist
   yet (FILESYSTEM / DRIVERS / NETWORK / USERSPACE as live doc areas) and
   would imply content the project cannot honestly show. Docs and FAQ keep a
   real in-page sidebar (doc list / categories) where actual content exists.
   Layout, cells and typography inside the pages follow the mockups.
10. **Search inputs omitted** (security page, changelog mockups). A search box
    with no backing index would be a dead control; static site ships no JS.
    May return with a real client-side index later.
11. **Accordion (FAQ) and mobile menu use native `<details>/<summary>`**
    instead of the mockup's JavaScript accordion — zero JS, keyboard
    accessible by default. Behavior (one panel per item) is slightly looser:
    multiple items can be open, which is also better for accessibility.
12. **Roadmap timeline node markers are squares, not circles** — the export's
    `rounded-full` dots contradict the design system's own "strictly sharp
    (0px)" shape rule; squares keep the blueprint language consistent.
13. **Status colors normalized to tokens.** Amber uses `#fbbf24` and Verified
    blue `#3b82f6` (from the export's own utility classes); Blocked/error text
    uses `#ffb4ab` on dark (the export's `error` token) because `#c31f29`
    fails contrast on `#050505` and is kept for borders only. Blue-grey
    `#7A8C99` was contrast-checked at ≈5.8:1 on `#050505` — no adjustment
    needed.
14. **Homepage architecture visual is an accessible layered list** (the
    `SYSTEM_ARCH_VIZ_RENDER` placeholder panel / three.js shader in the export
    is replaced by a semantic, honest layer stack with statuses — no WebGL,
    no JS, screen-reader readable).
15. **Footer year reads the build year (2026), not the mockup's 2024**, and
    footer links point at real routes (Experimental Status → /build,
    Source → /source, Kernel Documentation → /docs, Security Policy →
    /security).
16. **"Engineering" nav disclosure added** to the header for the routes the
    mockup header omits (engineering, releases, changelog, philosophy, FAQ,
    source), implemented as a no-JS `<details>` dropdown in the header's
    visual language.
