---
name: desktop-vs-web-paths
description: "Frontend layout: desktop crate `neoism` at neoism-frontend/desktop, shared crate `neoism-ui` at neoism-frontend/shared. A 'wave6 cutover' is lifting panels from desktop into shared; desktop now consumes many via neoism_ui::."
metadata:
  node_type: memory
  type: feedback
  originSessionId: f2240b31-eb5c-465c-8464-f39448e401db
---

Current frontend layout (verified 2026-06-03, supersedes the old
`frontends/neoism/` + `neoism-ui/` description):

- **Desktop** binary = crate **`neoism`** at `neoism-frontend/desktop/`.
- **Shared** UI = crate **`neoism-ui`** at `neoism-frontend/shared/`,
  imported as `neoism_ui::`. Used by BOTH desktop and web.

An in-progress "wave6 cutover" is lifting panels/widgets out of the old
desktop fork into the shared `neoism-ui` crate (docstrings say "Lifted
verbatim from `frontends/neoism/...`" / "after the cutover the widget
lives in the shared neoism-ui crate"). So the old "desktop does NOT use
the shared UI" rule is now only PARTIALLY true.

**How to apply:**
- It's a partial cutover — some panels are now shared and consumed by
  desktop (e.g. `neoism_ui::panels::{context_menu, buffer_tabs, island,
  tags_view, extensions_page}`), while others are still desktop-forked
  (e.g. file_tree at `neoism-frontend/desktop/src/editor/file_tree/`,
  agent pane partly at `neoism-frontend/desktop/src/neoism/agent/`).
- Before editing a panel/widget, check which copy desktop actually
  calls: `grep -rn "neoism_ui::...::X" neoism-frontend/desktop/`. If
  desktop imports the shared symbol, edit `neoism-frontend/shared/`;
  if it has its own local copy, edit that.
- Several files appear in BOTH trees during the cutover (git status
  often shows shared + desktop versions of the "same" panel touched
  together). Related: [[feedback_chrome_panel_style]].
