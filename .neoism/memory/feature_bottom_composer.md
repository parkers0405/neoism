---
name: Warp-style bottom command composer (shipped)
description: The sticky `>>>` composer is now an off-grid sugarloaf widget; block headers are the in-grid Warp-style strip
type: project
originSessionId: a22dfe78-6308-4520-8808-47ad28a168f6
---
The Warp-style command composer is implemented and live for the active terminal pane.

**Architecture:**
- New widget at `frontends/rioterm/src/renderer/command_composer.rs` (~430 lines). Pure sugarloaf primitives (`rounded_rect`, `rect`, `text_mut().draw`) — no `Row<Square>`, no terminal cells.
- The active terminal pane shrinks its renderable cell-row count by `command_composer.reserved_rows(cell_h)` so output never paints under the chassis. Inactive panes keep their full grid (composer is single-pane).
- Caret rect lives on `ComposerFrame.caret_rect` and gets surfaced to `current_terminal_block_input_cursor_rect` for damage tracking. The cell-grid cursor is suppressed while the composer owns input.
- Block-card headers stay in the grid (existing scroll-with-output composition keeps working); the verbose meta strip got replaced with `● <cwd>     <duration>s` — colored status dot (running=amber, ok=green, err=red).

**Why:** The user wanted pixel scrolling to stay correct, which requires the prompt to NOT eat PTY rows. Their critical phrase: "make this NOT on the terminal grid so we can keep our pixel scrolling right, make this like our status bar a SEPARATE element."

**How to apply:** Future composer work (hit-test for click-to-submit, completion popup overlay, per-block sugarloaf cards) goes in this same widget. `CommandBlockSnapshot` + `BlockStatusKind` + `command_block_snapshots()` are already exposed on `TerminalInputBuffer` for the upcoming per-block sugarloaf overlay pass — they're `#[allow(dead_code)]` until that lands.

**Status:** Composer + block-header polish landed and `cargo test -p neoism` passes (217/217). Per-block sugarloaf card overlays (rounded backings, status pills floating outside the grid) are NOT done — left as the next iteration since they need scroll-aware Y tracking.

**Gold-standard wrap pass (2026-07-06):** row reservation now prefers the render-measured `last_input_wrap` widths over the hardcoded 320/136px estimates (was clipping wrapped rows); `COMPOSER_MAX_INPUT_LINES` 6→10 with a `pane_rows/2` clamp (new `pane_rows` param threaded through `reserved_rows_for_input`/`terminal_reserved_rows_for_input`/`actual_chassis_height_for_input` — 9 call sites incl. wasm + chrome.rs); "↑/↓ N more" hidden-row indicators (surface-pill backed); desktop ArrowUp/Down use `input_visual_line_ranges` + `move_visual_up_in_ranges` like wasm (was falling into history and clobbering wrapped drafts).

**Round 2 (same day, after live feedback):** (1) continuation rows wrap to the FULL inner width — clamping every row at the run-chip column left a huge dead right margin; only chassis row 0 shares the run chip's line. (2) Scrolled window: the chevron prompt HIDES and the top chassis row becomes chrome-only (run chip + ↑ indicator); text starts one display slot down (`row_slot_offset`), chevron advance is computed from the fixed `PROMPT_CHEVRONS * font*0.62 + 6s` math so wrap widths never depend on whether the prompt painted. (3) THE UP-ARROW STUCK LOOP: `current_visual_range_index` (terminal_blocks/input.rs) and `line_for_byte` (command_composer/classify.rs) both moved to inclusive-end/first-match — a cursor clamped onto a soft-wrap boundary (end of row N == start of row N+1) previously re-resolved as row N+1 so every Up re-entered the same row; a cursor at a hard line's end matched NO row and fell to the LAST row. The two functions must stay convention-identical.
