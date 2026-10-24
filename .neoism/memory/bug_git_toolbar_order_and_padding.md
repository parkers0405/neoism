---
name: "Git toolbar icons and wallpaper padding slivers — fixed"
description: "Git toolbar ordered-vector icon fix; Tab hint removed but shortcut retained; opaque slot backing covers native padding without moving hit geometry"
type: "bug"
scope: "project"
origin: "screenshot follow-up"
created: "2026-10-21"
updated: "2026-10-21"
---

Fixed Git review toolbar icons invisible: plain Sugarloaf.line emits compositor order0, hidden behind GitDiffView opaque bg order20. Toolbar structural icons now font-independent ordered triangle strokes at order22. Same issue as missing-side hatching (never use unordered line for chrome above opaque fills). Removed visible [Tab] hint and its layout/hit slot; actual keyboardTab stillcyclesmodes. Native wallpaper slivers top/right were padding outside layout_rect, not diffbody clipping. In desktop screen/bridges/git_diff_view.rs render_git_diff_views paints opaque active-tab theme.bg backing below buffer-tabs ORDER_BG (backing order3). Backdrop starts from structural slot_rect, extends ONLY window-facing edges of visible-grid bounds into actual central chrome band (excludes sidebars/status), so no gap shows wallpaper and adjacent split/terminal remains untouched. Content viewport/hit/scroll/projected code geometry unchanged. Native helper tests cover DPI, single pane fill, internal split preservation/sidebar clamp. 23 shared review-view tests pass; native --bin neoism --tests cargo check passes (native geometry tests typechecked notexecuted); git diff --check passes. User explicitly wants Tab shortcut KEPT but hint REMOVED. No release/commit/livevisualvalidation.
