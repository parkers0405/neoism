---
name: "Table correctness and navigation overhaul"
description: "Unified table wrap/caret/hit/selection geometry, active-column reveal, undoable column menus, mode-aware cell projection/editing; skip frontmatter fences and Notes scrolloff parity."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-17"
---

# Markdown table correctness, frontmatter fences, Notes scrolloff

User reported caret stopping on hidden frontmatter `---`, Notes selection at viewport edges unlike file tree, clipped active table columns, no column deletion, bad wrapping/extra lines in Normal+Insert. Implemented integrated corrections, not a cosmetic table pass.

## Table geometry
- New `shared/src/editor/markdown/render/table_layout.rs`: pure source-preserving word-wrap ranges (visible char offsets, hard-wrapped URLs, repeated whitespace, line breaks, grapheme-safe combining/emoji clusters) + `reveal_column` whole-column horizontal follow with bounded margin/caret fallback.
- render/table.rs uses measured cell rows + exact hit stops for text, caret, selection, links, and clicks. Removed prefix-only caret rewrapping, guessed +1 offsets after every wrap, raw-text centering against cleaned-text heights. Text top-aligns inside cells. Cell clips constrain glyphs/underlines. Alignment markers (left/center/right) applied consistently through hit stops.
- Natural column widths bounded to viewport; spare width distributed; tall/large tables retain actual wrapped row heights instead of switching to fixed heights after 256 rows. Visible row range uses cumulative offsets. Grid rules meet row boundaries. Scrollbar has reserved space inside measured table height, not below it overlapping next block.
- Only keyboard/edit follow scrolls to whole current column. Wheel/thumb scrolling no longer changes the document's edit position and disables follow until navigation resumes.

## Source projection / cache
- Normal table cells use InlineSourceMap::for_table (renders markup + `<br>`). Active Insert cell uses table_edit (shows inline Markdown syntax but decodes table pipe escapes and cell line breaks).
- MarkdownPane::table_source_map/table_cell_revealed are shared by draw, measurement and navigation. MarkdownTableCellRect captures source_revealed at render time so pointer mapping uses the geometry actually drawn.
- MarkdownVirtualRenderState.measured_table_cell + MarkdownVirtualMeasureKey.table_cell track active (line,column). Moving cells on same row invalidates table measurement/cache; mode-only changes remain handled by reveal invalidation.

## Editing / data
- parse_table_cell_bounds now respects escaped pipes, matched backtick spans and Unicode whitespace; true formatting padding excluded. Shared cell parser used consistently for table structure. One-column tables supported. Only header delimiter row is structural: later dash-data rows remain editable data.
- Header hover `...` menu: Insert column before/after, Delete column. Last column explicitly offers Delete table. MarkdownTableAction public enum + ContextMenuAction::MarkdownTable wired desktop and web, including web menu routing. Deletion undoable, readonly guarded, preserves surviving values/alignment. Ragged rows padded before insertion; generated Column N headings avoid collisions.
- Home/End stay within current cell; Normal->Insert maps inside cell, never outside structural pipes. Tab/Shift-Tab traverse cells; Tab on last cell appends row. Enter adds row; Shift+Enter inserts cell `<br>`.
- Pasted multiline text/typed pipes inside cells become `<br>` / escaped pipes, not accidental Markdown rows/columns. Encoded line-break/pipe deletion is atomic and undoable. Inline code containing literal `<br>` is not decoded as break.
- Characterwise selection deletion within one table clears selected cell content while retaining grid delimiters and header separator; explicit linewise Vim deletion retains source semantics.

## Other requested fixes
- navigation::is_editable_line skips actual frontmatter fence pair in vim Normal/Visual; Insert may edit them.
- NotesSidebar::clamp_scroll now uses file_tree::SCROLL_OFF_ROWS capped to half viewport. Existing springs and GPU partial-row clipping retained.

## Verification / constraints
Added tests for source wrap offsets/graphemes, column follow, alignment, Unicode/escaped/code pipes, single-column edit/delete, delete undo/redo, multiline paste, raw/visible projections, active-cell measurement invalidation, selection grid preservation, cell Home/End/Tab, fences, and Notes scrolloff. Verification via cargo check --tests shared/desktop and wasm cargo check, NOT executed tests or live GUI verification. No release build. Notebook creation remains paused; do not re-enable it.
