---
name: "Markdown table/menu polish and notebook creation pause"
description: "Table controls/grid, spring-scrolled menus, Normal frontmatter + mini property completion, extensionless links, MD tab hover reveal; notebook creation UI/MCP paused by user."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-16"
updated: "2026-09-16"
---

# Table/menu/frontmatter/tab follow-up; notebooks creation paused

User requested Notion-like tables, genuine file-tree spring scrolling in slash menu, Normal-mode frontmatter parity + small supported-property completion, extensionless Markdown link rendering, disabling notebook creation, and agent-style Markdown tab hover reveal.

Implemented this pass:
- shared Markdown table renderer owns its single block handle (virtual draw_blocks skips generic Table registration/actions), precise handle hit rect, restrained header tint/grid, evenly distributes spare table width in shared measure/draw routine; append-row bar bottom, append-column bar right; interior row insertion only on left boundary gutter hover; no persistent accent insertion rule; neutral action buttons; copy hover-only; bottom padding reserved. Existing selection and source editing remain.
- ContextMenu owns CriticallyDampedSpring + web_time timestamp; move selection/wheel accumulate row lag and render ticks same 0.30s constant as tree. Draw fractional partial rows with list glyph clip, clip selected bg/cursor, animated hit testing/thumb; hover selection doesn't force scroll. Explicit redraw owners added desktop host/state.rs and shared chrome/content.rs.
- Frontmatter draw/measurement uses reveals_source_line instead of cursor-line equality, so Normal doesn't reveal YAML. Supported-property completion (title/icon/cover/tags) on empty/partial key inside existing frontmatter, Insert-only, no arbitrary YAML/LSP process. Existing icon/cover values retained. Picker smaller/bordered and constrained to editor; accepting records undo. Normal caret retained on rendered rows.
- Explicit Markdown links now accept nonempty extensionless destinations via rendered_link_target; inline_runs parses generic explicit links before bare-URL recognizer, so [MIT License](LICENSE) and [NOTICE](NOTICE) become labels with clickable targets, source maps consistent. Bare prose detection unchanged.
- User explicitly paused NOTEBOOK CREATION: removed New Notebook / Turn Folder into Notebook from Notes and explorer; removed palette create entry and availability. Existing Open Notebook and manifests remain. MCP notebookCreate schema and dispatch removed (unknown tool); existing read/list/addPage/movePage remain. Shared creation code retained for later. Integration/unit tests seed fixtures through shared creator rather than removed MCP. docs header warns feature creation paused. Do not re-enable without request.
- Markdown tabs use same 140px title cap and hover marquee as agent tabs, via shared buffer_tabs impl_core/impl_render; helper renamed compact_title_width. Other code tabs unchanged.

Regression coverage added for compact MD tabs, supported frontmatter completion/Normal no-reveal, extensionless label/target rendering and relative resolution, animated menu hit coordinates/stability, and MCP discovery excludes create. Verification uses cargo check --tests, no release build or executed tests. Multiple concurrent edits exist; do not revert unrelated changes.
