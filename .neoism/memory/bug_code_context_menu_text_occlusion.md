---
name: "Code context-menu broad text disappearance"
description: "Context-menu cut plus whole-syntax-run rejection erased surrounding editor text; use late rounded material and precise glyph clips"
type: "bug"
scope: "project"
origin: "user screenshot investigation"
created: "2026-10-21"
updated: "2026-10-21"
---

The LSP right-click menu broadly erased Rust code because desktop host/tabs.rs included context_menu.rect() in active_text_occlusion_rects and shared editor/code/render.rs::draw_text returned without drawing an entire syntax run whenever its bounds intersected a cut. Desktop host/run.rs already wraps context-menu rendering in Sugarloaf late overlay mode, so rectangular source suppression was redundant and destroyed rounded-edge background content. Fix removes that context-menu cut, wraps shared Chrome draw.rs context-menu render in late overlay mode (desktop parity), and replaces code run rejection with disjoint logical clip subtraction; unchanged glyph origins/shaping plus Sugarloaf endpoint snapping preserve partial glyphs at fractional DPI. Current desktop Screen calls shared code render directly (immediate Text); Sugarloaf VirtualCodeAdapter handles source tiles and has no menu occlusion input. Grid shaders clip to pane bounds; rendering order is grids -> normal UI quads -> normal Text -> late overlay material -> overlay Text. CPU regression in sugarloaf/src/renderer/cpu/tests.rs verifies opaque rounded menu over actual Rust text preserves every pixel outside its silhouette at 1/1.25/1.5/2 scales. Shared code render tests cover overlapping cuts, disjoint fragments and actual partial-glyph raster clipping through production draw helper. Do not drive UI or restart app to validate repository fixes.
