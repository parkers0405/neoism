---
name: "Markdown overlay clipping and keyboard scroll"
description: "Smooth symmetric Markdown single-line scrolling; pointer checkbox toggles preserve caret and viewport"
type: "bug"
scope: "project"
origin: "Fixed after user reported upward Arrow scrolling asymmetry and checkbox clicks jumping back to caret"
created: "2026-08-17"
updated: "2026-08-17"
---

Final upward-scroll asymmetry: ordinary ArrowUp/Down had a valid cursor rect from the prior frame, but virtual reveal still ran before drawing the new caret and could override the scrolloff target (especially upward) with a raw row-top reveal. `render_virtual` now records whether the caret was drawn last frame and only invokes source reveal when follow_cursor is true AND no prior caret was available. Normal one-line movement is therefore driven solely by measured scrolloff + Neovide spring; true offscreen jumps reveal on the following frame if needed.

Pointer task checkbox toggles must not claim cursor follow. `toggle_task_on_line` now takes a follow flag: mouse `toggle_task_at` passes false, keyboard `toggle_task_at_cursor` passes true. Mouse toggles preserve cursor line/column, scroll_y, target_scroll_y, and set follow_cursor false.
