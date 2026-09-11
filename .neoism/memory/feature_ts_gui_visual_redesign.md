---
name: "TS GUI visual redesign and exact fonts"
description: "User-corrected TS GUI visuals: OpenCode settings/Connect rows, FULL animated NEOISM letters, Geist + native Press Start 2P titles; 150 tests"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-10"
updated: "2026-09-10"
---

## Latest visual requirements (supersedes initial single-N splash notes)
User rejected first TS GUI as generic/ugly. Wants close OpenCode desktop/app geometry and settings structure, NOT a merely similar architecture or long stacked preferences form. User forbids Playwright/browser tooling; source inspection + tests/HTTP checks only. OpenCode source pinned 859106eb17d5b840475f5e4b78e64c9622f8750e, current settings-v2 and shared v2 composer.

Home MUST full NEOISM six-letter wordmark above composer with independent per-letter hover/lift/shimmer, click squash/ripple matching Rust desktop. NOT single N and NOT a font-rendered text approximation. Original18 SVG depth paths, geometry/constants generated with scripts/generate-wordmark.mjs. Wordmark export Identity.tsx, motion helpers/Wordmark.css tests. Native gap44px. Removed all marketing title/subtitle/footer text. Logo() retained only reusable legacy export, App uses Wordmark.

Typography explicitly Geist UI/body + Press Start 2P titles. User corrected font source: Neoism agent-side-panel, NOT Synapse. Native FONT_PRESS_START_2P points sugarloaf/src/font/resources/PressStart2P/PressStart2P-Regular.ttf, copied exact with OFL; generated font id press-start-2p weight400 (avoid synthetic bold). Geist variable100-900 official pinned source vendored offline, id geist. Inter additions were removed after user override. Existing system-sans maps Geist first; new default font geist. --font-heading set to Press Start 2P. Page titles11-12px, Recents/right-sidebar headings10px. Body13px Geist440; code retains mono.

Settings rebuilt Settings.tsx + ProviderDirectory.tsx + ProviderConnections.tsx + settings-provider.css: 980x600 max, radius6, 240px left nav (mobile144), dedicated General/Appearance/Servers/Providers, provider Connected/Popular rows with right edge +Connect, Accounts action for multiauth, branded SVG source licensed from OpenCode. Connect drilldown max640x512 with back to provider list, credential methods/fields separate from overview. All account/OAuth/rename/default/delete/selected-billing safeguards preserved. App does NOT pass initialProviderId to Settings gear (otherwise it bypasses settings and opens auth), but still passes it to anchored /connect.

Shell CSS now stable240 sidebars,44 topbar, neutral surfaces, compact rows. Composer10 radius, editor60-180, controls44 row/28 buttons, send square28 radius6. Pickers anchored above8px radius10. No blurred modals or huge radii. appearance.ts defines real neutral 'neoism' default plus101 native palettes; previously invalid neoism silently fell back to black pastel_dark. Semantic --surface-1/2/3 match #242424/#2e2e2e/#3a3a3a default with bg#161616 fg#fafafa muted#aeaeae; native palettes map same structure. Added palette tests. Title styles don't pixelate body/controls.

Verified production TS/Vite build,150 tests13 suites, native theme/command/font/wordmark assets checks, whitespace. Six bundled font families13 faces1.52MB. HTTP200 actual dev page/fonts/Identity/Settings at http://127.0.0.1:5176. No visual browser verification. Existing backend/commands unchanged in this redesign. Full standalone task original details in project_standalone_typescript_gui.md.
