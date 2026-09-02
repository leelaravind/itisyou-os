---
name: ITISYOU OS Architectural Framework
colors:
  surface: '#131313'
  surface-dim: '#131313'
  surface-bright: '#3a3939'
  surface-container-lowest: '#0e0e0e'
  surface-container-low: '#1c1b1b'
  surface-container: '#201f1f'
  surface-container-high: '#2a2a2a'
  surface-container-highest: '#353534'
  on-surface: '#e5e2e1'
  on-surface-variant: '#c4c7c8'
  inverse-surface: '#e5e2e1'
  inverse-on-surface: '#313030'
  outline: '#8e9192'
  outline-variant: '#444748'
  surface-tint: '#c6c6c7'
  primary: '#ffffff'
  on-primary: '#2f3131'
  primary-container: '#e2e2e2'
  on-primary-container: '#636565'
  inverse-primary: '#5d5f5f'
  secondary: '#b6c9d7'
  on-secondary: '#21323d'
  secondary-container: '#384955'
  on-secondary-container: '#a5b8c5'
  tertiary: '#ffffff'
  on-tertiary: '#68000b'
  tertiary-container: '#ffdad7'
  on-tertiary-container: '#c31f29'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#e2e2e2'
  primary-fixed-dim: '#c6c6c7'
  on-primary-fixed: '#1a1c1c'
  on-primary-fixed-variant: '#454747'
  secondary-fixed: '#d2e5f3'
  secondary-fixed-dim: '#b6c9d7'
  on-secondary-fixed: '#0b1d28'
  on-secondary-fixed-variant: '#384955'
  tertiary-fixed: '#ffdad7'
  tertiary-fixed-dim: '#ffb3ae'
  on-tertiary-fixed: '#410004'
  on-tertiary-fixed-variant: '#930014'
  background: '#131313'
  on-background: '#e5e2e1'
  surface-variant: '#353534'
typography:
  display-lg:
    fontFamily: Geist
    fontSize: 48px
    fontWeight: '300'
    lineHeight: '1.1'
    letterSpacing: -0.02em
  headline-md:
    fontFamily: Geist
    fontSize: 24px
    fontWeight: '400'
    lineHeight: '1.2'
    letterSpacing: -0.01em
  body-base:
    fontFamily: Geist
    fontSize: 14px
    fontWeight: '400'
    lineHeight: '1.6'
    letterSpacing: 0em
  mono-label:
    fontFamily: JetBrains Mono
    fontSize: 12px
    fontWeight: '500'
    lineHeight: '1'
    letterSpacing: 0.05em
  mono-data:
    fontFamily: JetBrains Mono
    fontSize: 13px
    fontWeight: '400'
    lineHeight: '1.5'
    letterSpacing: 0em
  status-tag:
    fontFamily: JetBrains Mono
    fontSize: 10px
    fontWeight: '700'
    lineHeight: '1'
    letterSpacing: 0.1em
spacing:
  unit: 4px
  gutter: 24px
  margin: 40px
  container-max: 1440px
---

## Brand & Style
The design system embodies a "Technical Minimalist" aesthetic, prioritizing structural clarity and engineering precision over decorative elements. It is designed for an independent operating system environment where the UI acts as a transparent window into system architecture.

The style leans into **Modern Minimalism** mixed with **Functional Industrialism**. It avoids the softness of consumer SaaS in favor of sharp lines, high information density, and deliberate negative space. The goal is to evoke the feeling of a high-end engineering workstation—cold, focused, and reliable. Every element must feel like it has a specific functional purpose; if a component doesn't serve a technical need, it is omitted.

## Colors
The palette is rooted in deep obsidian tones to minimize eye strain and maximize the impact of data.
- **Base:** `#050505` serves as the global background.
- **Surface:** `#0C0C0C` and `#121212` are used for containers and structural blocks.
- **Borders:** `#1A1A1A` is the primary tool for creating depth, replacing shadows.
- **Accents:** Restrained use of Cold White (`#F2F2F2`) for primary data and Blue-Grey (`#7A8C99`) for secondary metadata. 
- **Functional Colors:** Use Amber sparingly for 'Experimental' or 'Research' states, and Red for 'Blocked' or system errors.

## Typography
The system utilizes two distinct typefaces to separate UI navigation from system data:
- **Geist:** Used for all primary interface elements, headings, and long-form documentation. Its geometric precision fits the engineering-first narrative.
- **JetBrains Mono:** Reserved for technical metadata, kernel outputs, version numbers, and status indicators. It should always appear in uppercase when used for tags to enhance scannability.

Avoid heavy weights. Stick to Light (300) and Regular (400) for most UI, using Medium (500) only for small labels or buttons.

## Layout & Spacing
This design system employs a **Fixed Grid** approach built on a 4px baseline. 
- **The 12-Column Grid:** Desktop layouts use a strict 12-column grid with 24px gutters. 
- **Structural Lines:** Instead of spacing alone, use 1px vertical and horizontal lines (`#1A1A1A`) to subdivide the screen into functional quadrants, mimicking technical blueprints.
- **Margins:** Generous outer margins (40px+) emphasize the "independent" and "premium" nature of the OS, preventing the UI from feeling cluttered.
- **Alignment:** All content must be hard-aligned to the grid. Avoid centering elements; favor top-left alignment for a logical, document-based flow.

## Elevation & Depth
Elevation is expressed through **Tonal Layering** and **Structural Outlines** rather than physical shadows.
- **Level 0:** Global Background (`#050505`).
- **Level 1:** Content Areas (`#0C0C0C`) defined by 1px borders (`#1A1A1A`).
- **Level 2:** Active Overlays/Modals (`#121212`) with a subtle 20px backdrop blur to maintain context of the layer beneath.
- **Interaction:** Hover states should not lift the element; instead, change the border color to `#333333` or shift the background tone slightly.

## Shapes
The shape language is strictly **Sharp (0px)**. 
Rounded corners are perceived as "friendly" and "consumer-oriented," which contradicts the technical seriousness of this system. Rectilinear shapes reinforce the grid and the feeling of modularity. Use 45-degree chamfered corners sparingly for specialized "Experimental" tags to denote a break from the standard grid.

## Components
- **Buttons:** Rectangular with 1px borders. Primary buttons use a ghost style (white border, transparent fill) until hovered. Use JetBrains Mono for button labels.
- **Status Indicators (Tags):**
    - `Planned`: Bordered, no fill, grey text.
    - `Research`: Dotted border, amber text.
    - `In Development`: Solid border, blue-grey text.
    - `Implemented`: Solid white border, white text.
    - `Verified`: Solid blue border, blue text.
    - `Experimental`: 45-degree corner cut, amber background, black text.
    - `Blocked`: Solid red border, red text.
- **Input Fields:** Bottom-border only by default. Becomes a full 1px box on focus. No glowing shadows.
- **Cards:** Do not use cards with shadows. Use "Cells" defined by 1px dividers that span the full width of their container.
- **System Logs:** Monospaced blocks with a slightly darker background (`#080808`) to differentiate code/logs from UI text.
- **Breadcrumbs:** Use forward slashes (`/`) in JetBrains Mono to mimic file paths (e.g., `SYSTEM / KERNEL / ARCH`).