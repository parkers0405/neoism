---
name: "Agent partial text selection geometry"
description: "Agent text selection uses measured grapheme caret stops instead of whole-line average widths"
type: "bug"
scope: "project"
origin: "session implementation"
created: "2026-08-28"
updated: "2026-08-28"
---

Agent transcript partial mouse selection was fixed across shared state and desktop rendering.

Root cause: `slice_line_by_x` divided whole-line width by character count. Highlights used raw pointer pixels, but clipboard slicing used this average-width interpolation, so proportional/styled/narrow glyph runs often mapped both drag endpoints to the same synthetic character boundary; users had to drag across most of a sentence. Additional affected areas either registered wrapped text as one coarse row or did not register rows at all, and tool/link pointer-down actions fired before selection.

Current architecture:
- Shared `panels/agent_pane/selection_model.rs` defines `SelectableLine` and UTF-8 `SelectableCaretStop { byte_offset, x }`.
- Selection points store row identity (`content_y` + `row_x`), exact byte offset, and snapped x. Clipboard extraction slices exact byte ranges; table cells sharing y order by x.
- `view/draw.rs::measured_caret_stops` measures grapheme advances and normalizes each run to its painted width.
- Agent Markdown builds caret stops using each inline segment's actual bold/italic/code/link styling.
- Exact selectable rows are registered for normal Markdown, fenced code, Mermaid raw source, wrapped tool titles, tool body rows, and tool todo rows.
- Code and these surfaces paint highlights from the same snapped endpoints used for clipboard extraction.
- Desktop pointer-down now lets registered text win over link/tool actions. Plain clicks replay link open/copy or tool toggle on release; drags select without prematurely activating.
- Nearest-row fallback now considers x as well as y and hit testing prefers the latest painted row.

Tests: new proportional test selects exactly `ii` from synthetic `WW ii` unequal caret widths in both shared model and desktop pane. `cargo test -p neoism-ui panels::agent_pane --lib` passed 460 tests, focused desktop transcript selection tests passed 3/3, and `cargo check -p neoism-ui -p neoism` passed. Existing unrelated warnings remain.

Residual architecture gap: reusable diff-card body rows/diagnostic footer still need a selection callback from `widgets/diff_card.rs`; web link/tool click-vs-drag release deferral remains separate from desktop, though exact shared caret selection works on web ordinary text.
