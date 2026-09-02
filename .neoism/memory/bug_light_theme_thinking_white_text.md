---
name: bug_light_theme_thinking_white_text
description: Agent thinking title + body rendered near-white on light themes (retro_95); fixed by branching on IdeTheme::is_dark()
metadata: 
  node_type: memory
  type: project
  originSessionId: 61dc832e-208d-41c5-a0b4-dbc86d0b1599
---

**Symptom:** in the only light-mode look pack (retro_95), the agent pane's "Thinking"/reasoning TITLE and BODY paragraphs rendered near-white → unreadable on the light panel.

**Root cause:** the reasoning styling was tuned for dark themes and never consulted background luminance. Body used `theme.white` as its base (retro_95 `white` = `#f5f4ef`, near-white); title used `theme.u8_alpha(theme.yellow, 0.6)` which blended the amber toward the light `panel_bg` until it washed out.

**Fix (landed 2026-07-16, cargo-check clean, branch better_workspace):** route both through the existing `IdeTheme::is_dark()` (primitives/ide_theme.rs:422 — Rec.601 luma of `bg` < 128). Theme-aware, NOT hardcoded black; dark themes render identically.
- `panels/agent_pane/view/markdown.rs` (~1343, `render_markdown_blocks`, `body_muted`/reasoning path only): `let reasoning_dim = if theme.is_dark() { theme.white } else { theme.fg };` then use `reasoning_dim` for `body_color` and `u8_alpha(reasoning_dim, 0.6)` for `body_text_color`. On light = `theme.fg` (#1a1a1a) → dark headings + medium-gray dim paragraphs. Non-reasoning markdown already used `theme.fg` (unaffected — `body_muted` is set only by the reasoning render path).
- `panels/agent_pane/view/assistant.rs` (~122, `render_reasoning_message_with` title): `color: if theme.is_dark() { u8_alpha(yellow,0.6) } else { u8(yellow) }`. Light = full-opacity `theme.yellow` (#a87900 dark amber) → readable.

Both in shared crate `neoism-ui`; desktop agent pane renders through it. Pattern: any agent-pane color must branch on `theme.is_dark()` (or a theme token), never assume dark. Related: [[feature_look_packs]] (retro_95 is a light look pack), [[feature_cursor_style]].
