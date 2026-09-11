---
name: "TS GUI must preserve native identity"
description: "Latest TS GUI rules: native Pastelbeans not gray; literal Apple-blue/no-name bubbles; Geist + JetBrains code; Obsidian tabs, continuous models, native activity"
type: "feedback"
scope: "project"
origin: "user"
created: "2026-09-10"
updated: "2026-09-10"
---

User explicit corrections while iterating standalone TS GUI:
- Copy OpenCode/Obsidian STRUCTURE, not gray palette. User strongly rejected synthetic neutral gray 'neoism' theme. Removed it; GUI defaults to native Pastelbeans (actual ~/.config/neoism/config.json appearance.theme verified 'pastelbeans'), maps native bg/fg/surface/hover/border/accent directly. Migrate only old synthetic neoism preference; preserve explicit native theme selections. 101 native themes only.
- Splash FULL six-letter NEOISM original SVG layers with independent native hover/click animations, not single N, text-font approximation, slogans, subtitles.
- Regular/UI font default Geist; separate Code font selector/default JetBrains Mono. P2 titles = exact Press Start 2P from Neoism Rust agent sidebar (NOT Synapse); no synthetic bold on normal headings.
- Ordinary role:user messages must be Apple-blue #007aff with white text, compact right-aligned SMS bubbles/tail. NO author name above bubble. Display names ('parkersettle' native vs 'You' web) are NOT reliable remote/local identity. Do not infer ownership from those names or preserve a visible heading. Runtime cards excluded.
- Tabs Obsidian-style contiguous top chrome connecting sidebar, inactive same chrome, active joins chat bg with rounded top corners, no white underline and no floating pills. Titles normal UI font, ellipsis with close space. Wheel (vertical/horizontal) scrolls tabs. Left sidebar desktop collapse control; remove Connected label in pinned profile.
- Composer floating over chat canvas, shared centered content width and meaningful side gutters; project/hints footer pinned even when input contents overflow; animate home->bottom docking. Native curved skirt with agent/model/thinking colored controls. Clicking an already-open chip closes picker; another chip switches.
- Model chooser search on top + recent models + provider groups, continuously scrollable/virtualized. NO Next/Previous page UI. Exclude hidden/internal and subagent-only agents using native flags; Build/Plan + primary user agents.
- Native Crafting/Pondering glyph scramble/wave/ocean dots, native thinking style, task-specific cards, real file diffs, semantic prose colors, actual Tree-sitter capture colors from theme.
- Runtime job/subagent envelopes must become compact cards, never user text dumps. Current server prefix 'Agent runtime notification', historical prefix Neoism; match shared kind marker. Strip transport suffix '(@explore subagent)' from confirmed runtime card titles, not genuine user body text.
- Reopened history should omit old thinking/tools; new live work still visible. Automatic upward-scroll history paging must anchor reading and not cascade. All actual async loads use animated shape-specific skeletons, not stale wrong-scope content.
- No Playwright/browser tooling: inspect source, test hooks/SSR/Happy DOM, real WASM parsing, HTTP read-only smoke.

Latest implementation currently still in flight for click-performance/model list/history suppression/ordering. Parent added codefont prefs+CSS, useHistoryPagination hook/tests, tab wheel scrolling. Do not claim finished until current agents and integration tests complete.
