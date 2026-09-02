---
name: bug-presence-caret-one-line-off
description: Nvim remote-presence caret renders one line off while the peer types — statically unexplained; NEOISM_CARET_LOG triage landed
metadata: 
  node_type: memory
  type: project
  originSessionId: f68e5b0c-e85e-4e1d-a759-93fe8315bea1
---

Cross-host workspace sharing: the other user's live typing in nvim looks right but their presence caret sits one row off (reported 2026-07-05, screenshot).

**Static analysis (2026-07-06) found every hop consistent:** publisher `editor_presence_line = win_viewport.curline` (0-based, both parses identical: `nvim_events.rs parse_win_viewport` and daemon `nvim.rs "win_viewport"` arm); painter `remote_carets.rs:144 output_row = cue.line - topline - source_line_offset` matches the working inline-diagnostics math (`render/mod.rs ~4236`); geometry `panel_rect[1]+margin.top == visible_grid_top` per `chrome_policy::grid_panel_chrome_geometry` (EDITOR_BUFFER_ABOVE=1 already folded into panel_top). CRDT text apply is content-diff (`apply_authoritative_text_to_nvim` lua) — can't shift lines without corrupting.

**Why:** an off-by-one that survives consistent static math needs a live repro; two nvims (one per host) converge via CRDT, so publisher-vs-painter is the remaining split.

**How to apply:** run the observer side with `NEOISM_CARET_LOG=1` (env-gated tracing in `desktop/src/screen/render/mod.rs` remote-caret block, target `neoism::presence_caret`). Park the LOCAL cursor on the same buffer line the peer is typing on: if `cue_lines != local_curline` → publisher side (peer's curline/CRDT hop); if equal but the beam paints wrong → this painter's transform (`row_shift`/`scroll_off`/`pane_top` are all in the log line). Related: [[bug-inline-diag-lens-scroll]].
