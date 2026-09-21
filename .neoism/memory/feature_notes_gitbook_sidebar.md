---
name: "feature_notes_gitbook_sidebar"
description: "Implemented GitBook Alt+N takeover, notebook page breadcrumbs, search, and heading outline."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-21"
updated: "2026-09-21"
---

---
name: "feature_notes_gitbook_sidebar"
description: "Implemented GitBook Alt+N takeover, notebook page breadcrumbs, search, and heading outline."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-21"
updated: "2026-09-21"
---

# Native GitBook notes: Alt+N notebook takeover

Implemented 2026-09-21.

- Notebook folders are detected from a `notebook.json` child in the existing local/daemon notes listing.
- In the vault list, notebooks render with a dedicated notebook/book icon and no chevron; their descendants stay hidden.
- Click/Enter replaces Alt+N with a notebook-scoped hierarchy. The explicit top back row is keyboard-selectable: ArrowUp from the first page selects it, ArrowDown returns to the first page, and Enter exits the notebook. A divider and notebook title separate it from the page hierarchy.
- Inside the notebook, Markdown page labels hide `.md`, `.markdown`, and `.mdx` case-insensitively at presentation time. Only expandable folders show right-edge chevrons (`>` closed, down chevron open), matching GitBook navigation. Selection keeps the accent/left-bar treatment after editor focus moves.
- Local page activation attaches the existing `NotebookBinding`; daemon-backed joined pages continue through ordinary remote Markdown because guests cannot read host-local manifests.
- Shared Chrome/web and desktop input paths both implement enter/back behavior.
- `documentation_notebook/view.rs` no longer reserves or paints an editor-side rail; notebook-bound pages receive the full Markdown rectangle unless the heading outline is open.
- Notebook-bound Markdown breadcrumbs include a right-aligned search field and outline button. Search invokes the existing Markdown incremental search mode rather than a second text-input path.
- The outline button toggles `DocumentationNotebook::contents_open`. The right-side, pane-local heading panel reuses the existing outline parser, active-section state, hover/click animation, scrolling, and click-to-reveal behavior; it has no title row and closes without retaining stale hit geometry.
- Notebook creation is exposed in the Alt+N right-click menu as `New Notebook` (`b` shortcut). It creates an Untitled Notebook transactionally, refreshes the tree, and enters the new sidebar navigator rather than the retired editor rail. Local only; joined remote mutation remains unavailable.
- Notes MCP re-exposes mutating `notebookCreate` (denied in plan mode). Inputs: vault-relative `path`, optional `title`; creates `notebook.json` and ordinary `overview.md`. Fixed `checked_path` so existing manifest files do not gain a trailing empty component.
- Regression tests cover notebook takeover/back focus, Markdown display labels, breadcrumb control cleanup, MCP notebook creation/reordering, and complete Notes MCP advertisement.
- Verified with `cargo check -p neoism-ui -p neoism`, the focused regression tests, and `git diff --check`.
