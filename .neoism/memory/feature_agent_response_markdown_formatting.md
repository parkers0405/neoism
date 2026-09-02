---
name: "Agent response Markdown formatting"
description: "Agent Markdown now preserves list hierarchy and semantic prose, matching OpenCode-style readability"
type: "feature"
scope: "project"
origin: "session implementation"
created: "2026-08-28"
updated: "2026-08-28"
---

Agent assistant Markdown formatting was upgraded in `neoism-frontend/shared/src/panels/agent_pane/view/markdown.rs` and `markdown/inline_style.rs`.

Root cause: agent prose used a physical-line custom IR where `Bullet(Vec<String>)` discarded list marker, numbering, depth, and task state; the renderer intentionally drew no marker with zero indentation. It also split ordinary hard-wrapped paragraphs into independent blocks and heuristically converted sentences containing ` - ` into lists.

Current invariants:
- `AssistantMarkdownBlock::ListItem` preserves `AssistantListMarker` (unordered, ordered source marker, task checked state), nesting depth, and wrapped lines.
- Agent list parsing reuses `widgets::markdown::parse_line`, keeping editor/agent syntax aligned.
- Unordered markers, ordered labels, and task boxes render in a semantic gutter; nesting changes indentation; wrapped lines align under item text.
- Consecutive list items use compact 2px block gaps; other blocks retain normal spacing.
- Physical prose lines are joined into semantic paragraphs while fences, tables, headings, quotes, lists, dividers, and indented list continuations remain structured.
- The old `expand_inline_bullets` sentence heuristic was removed.
- Horizontal rules render explicitly.
- Inline parser/render supports italic, bold-italic, underscore strong, and styled link labels in addition to existing bold/strike/code/link behavior.

Regression tests cover semantic paragraph joining, nested/ordered/task list metadata, emphasis marker stripping, and exact card-height gap accounting. Verification: `cargo test -p neoism-ui markdown::tests` passed 160 tests; `cargo check -p neoism-ui` passed. Existing specialized code/table/Mermaid/tool/diff/reasoning renderers were not replaced.
