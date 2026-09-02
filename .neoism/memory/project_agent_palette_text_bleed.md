---
name: "Agent palette text bleed"
description: "Alt+P palette uses its live animated visual bounds as an agent-only Sugarloaf text occlusion on desktop and shared/web"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-11"
updated: "2026-08-11"
---

---
name: "Agent text bleeding through command palette"
description: "Alt+P palette uses its live animated visual bounds as an agent-only Sugarloaf text occlusion on desktop and shared/web"
type: "bug"
scope: "project"
created: "2026-08-11"
updated: "2026-08-11"
---

## Symptom

When Alt+P opened over an Agent pane, composer text and status-chip labels could phase through the command-palette material.

## Fix

`CommandPalette::active_visual_rect` is the single source for the palette's actual painted geometry, including its opening pop-scale and vertical offset. The renderer itself and text occlusion now use that same geometry.

Desktop adds this rect only to the Agent pane's existing text-occlusion list in `screen/bridges/agent.rs`. Shared/web passes it to the Agent pane and terminal splash in `chrome/draw.rs`.

This uses Sugarloaf's partial text carving: only glyph fragments intersecting the palette are removed. The underlying pane is not suppressed and no black/fullscreen backing rectangle is introduced. The palette remains in the late overlay pass.

## Verification

- `cargo check -p neoism-ui -p neoism`
- Focused `visual_rect_tracks_the_surface_used_for_text_occlusion` test
- `cargo fmt --all -- --check`
- `git diff --check`
