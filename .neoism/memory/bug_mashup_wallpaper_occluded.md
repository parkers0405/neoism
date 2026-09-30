---
name: "Mashup wallpaper pane occlusion — FIXED"
description: "Mashup wallpaper only top/bottom due pane bg fills; fixed all central surfaces + parked PTY suppression"
type: "bug"
scope: "project"
origin: "coding-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

---
name: bug-mashup-wallpaper-occluded
metadata:
  node_type: memory
  type: bug
---

Mash Up Pack wallpapers were visible only in top/bottom window bands because wallpaper renders first and later pane-sized `theme.bg` quads covered the middle. Fixed by removing only top-level body fills from shared code, legacy+virtual Markdown, NeoDraw/graph, Extensions, Tags, NeoWorld outer body, and focused/unfocused Chrome file surfaces. Local materials (cards, code blocks, controls, NeoWorld room) remain opaque. Desktop `PanelFrame` now carries `has_non_terminal_surface`; `emit_and_present_grids` skips the resident PTY grid for document/page panes, preventing stale terminal glyphs or ANSI backgrounds beneath transparent bodies. Agent pane already had this material-free contract. No-wallpaper appearance remains unchanged because Sugarloaf clear color is theme bg. Verified with `cargo check -p neoism-ui -p neoism` and `git diff --check`.
