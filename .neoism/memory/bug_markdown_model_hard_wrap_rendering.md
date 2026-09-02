---
name: "Markdown model hard-wraps rendered as ugly extra lines — fixed"
description: "Live Preview reflows model hard-wrapped Markdown soft breaks without changing source; source-accurate interaction maps included."
type: "bug"
scope: "project"
origin: "agent"
created: "2026-08-02"
updated: "2026-08-02"
---

---
name: Markdown model hard-wraps rendered as ugly extra lines — fixed
description: Live Preview now projects CommonMark soft breaks into paragraph reflow while preserving source, hard breaks, and source-accurate clicks.
type: bug
created: 2026-08-02
updated: 2026-08-02
---

# Markdown model hard-wrap rendering

Model workspace edits and notes writes preserve Markdown verbatim. Models commonly hard-wrap prose with physical newlines. Neoism's live-preview renderer incorrectly laid each physical line out independently, producing Shift+Enter-like rows and isolated words at viewport widths different from the model wrap width.

Fixed renderer-side, not by mutating files:

- `neoism-frontend/shared/src/editor/markdown/render/virtualized/inline_layout.rs` projects contiguous plain paragraph source lines into one display paragraph.
- CommonMark soft breaks become one reflowable display space.
- Trailing two spaces and odd trailing backslash remain hard visual breaks, with source markers hidden in preview.
- Blank lines and Markdown block boundaries remain boundaries.
- Virtual measurement and drawing consume the same projection; legacy fallback mirrors it.
- Paragraph hit maps map each rendered caret boundary back to the original physical source line/byte column, including hidden inline syntax and synthetic spaces.
- Insert-mode active lines split out for exact raw source editing; Normal/Visual cursor placement and selection work across collapsed paragraphs.
- Original source and CRDT text are never normalized.

Verification: `cargo test -p neoism-ui collapsed_paragraph_click_targets_later_physical_source_line --lib`, `cargo test -p neoism-ui inline_layout_tests --lib` (6 passed), `cargo check -p neoism-ui`, and targeted `git diff --check` all pass. Existing unrelated neodraw test warning remains.
