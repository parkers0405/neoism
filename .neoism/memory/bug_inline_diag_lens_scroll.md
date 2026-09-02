---
name: bug_inline_diag_lens_scroll
description: inline diagnostic lens (red error chips) vanish after Ctrl-D/Ctrl-U scroll until buffer reopen; spring residual vs scrollback-ring origin
metadata: 
  node_type: memory
  type: project
  originSessionId: c92e2fa6-7b2b-493f-9c31-7406a2386cce
---

Two separate systems draw nvim "errors": the **undercurl** is nvim's own grid highlight (`DiagnosticUnderlineError` in `rio/theme.lua`) baked into terminal grid cells — survives scroll. The **red message chip** is Rust chrome, `shared/src/panels/inline_diagnostics.rs`, a per-frame sugarloaf overlay (overlay batches reset every present), re-emitted from `desktop/.../screen/render/mod.rs` (~line 4014) and positioned by `item.lnum - 1 - editor_viewport_topline`.

**Bug (fixed 2026-06-15):** the lens subtracted `editor_source_line_offset` (the smooth-scroll spring's integer floor). The desktop editor keeps TWO scroll quantities — the spring offset AND the scrollback-ring origin (`scrollback_origin`). After a `grid_line`-redraw scroll (Ctrl-D / Ctrl-U — big jumps redraw via grid_line, not grid_scroll; see `nvim_events.rs` WinViewport comment) the spring settles on a **non-zero residual** while the ring origin shifts to compensate. The grid samples through the ring so it stays correct; the lens double-counted the spring, shifting every chip out of `[-1, visible_rows]` → chips vanish until reopen (BufEnter republishes + resets the spring). Data was never lost — pure positioning.

**REGRESSED + re-fixed 2026-07-06:** the `- editor_source_line_offset` subtraction came back (lost during the Rust-LSP migration refactor) at `desktop/src/screen/render/mod.rs:~4261`; same symptom (chips draw on load, vanish after Ctrl-D/Ctrl-U scroll-past-and-back, never redraw until buffer reopen). Re-applied `let row = source_y;`. Only the DESKTOP renders inline diagnostic chips (`inline_diagnostics.render` has one caller); web/wasm has no inline-chip path, so no parallel fix needed. Data always lived in the persistent `current.editor_diagnostics` context field — pure positioning.

**Fix:** `let row = source_y;` (drop the `- editor_source_line_offset`). The ring origin and spring cancel for on-screen position — in steady state the line at screen row 0 is always nvim's `topline`, so `screen_row == source_y`. Sub-row glide still rides `editor_pixel_offset_y` (which is 0 at rest). Tradeoff: during a fast multi-row animation the chip snaps to its line instead of gliding the integer rows; deeper fix would be rebasing the spring to 0 at rest (scroll-model change, riskier). The grid's own `editor_source_line_offset` uses (rebuild_row at ~3495/3573) are correct because they go through the ring.

Related: [[project_workspace_root_model]], [[bug_md_scroll_bop]]. The composer-on-draw-tabs fix shipped same session added `Context::has_non_terminal_surface()` (includes `draw` + `neoism_extensions`, which the old per-gate `editor/markdown/agent/tags` chains omitted).
