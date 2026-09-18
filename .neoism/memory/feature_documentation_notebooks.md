---
name: "Documentation notebooks (Scalar-style, one tab)"
description: "First desktop/local folder notebooks: one tab, existing MD contexts, rails/history/page management; link fixes. Pending grouped tab drag, web/remote, search, chooser."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-16"
updated: "2026-09-16"
---

# Documentation notebooks: first desktop/local integration

User wants Scalar/docs-style bound Markdown collections: a special folder, one Neoism buffer tab, ordered pages, left contents, existing Markdown center, right heading outline, previous/next and history. Not Jupyter (.ipynb).

## Implemented
- Shared model `neoism-frontend/shared/src/editor/documentation_notebook.rs` and `documentation_notebook/view.rs`: `notebook.json` title/pages manifest; simple path or named {path,title,section}; external references; normalized duplicate rejection; ordered/history navigation; collapsible sections; modifications staged via create_new temp + rename and external-change conflict checking.
- `MarkdownPane.documentation_notebook: Option<NotebookBinding>` contains manifest path + Arc<Mutex<DocumentationNotebook>>. Existing live Markdown contexts retained per page. `tab_path()` projects stable manifest identity; `is_active_tab_path` locates the session's active member for tab activation. Do NOT replace the document's actual path with notebook.json (CRDT/save routes must remain per file).
- Desktop bridge `screen/bridges/markdown/documentation_notebook.rs`: create/open, bounded recursive folder import, page navigation, history, add/create/reference pages, move up/down, dirty aggregation/close guards, file-tree rename/move rebinding, last-page resume in config/notebooks/<sha256>.json.
- Real layout space reserved for left/header/footer; `render/virtualized/surface.rs` reserves right heading gutter when wide. Real Markdown render rect feeds selection viewport bounds.
- Palette: Create Documentation Notebook from Folder, Open Documentation Notebook. Explorer folder context menu: Turn Folder into Notebook / Open Notebook. Normal open of notebook.json routes to notebook; folder keyboard activation recognizes it.
- Link to Markdown File palette action and desktop file_links.rs: existing outside-vault path input, inserts escaped standard relative/file-URL link. Shared link resolver fixes same-page headings using unsaved lines (uncached), percent decoding, angle-delimited destinations, native file URLs and home expansion. The user's exact broken 'Prompt admission and API' destination was never supplied, so do not claim exact diagnosis.
- Regression coverage for navigation/history, invalid/duplicate manifest pages, outside paths, rename rebasing, persistence conflicts, tab identity vs live page state, live headings and link encoding. Verification is cargo check (tests compiled, not executed), per user's build workflow. No release builds.
- Guide: docs/documentation-notebooks.md.

## Initial-scope limits / next increments
- Desktop/local only. Shared renderer exists but wasm/remote host wiring not done; new palette actions hidden for wasm. Native URL/path APIs cfg-gated so web compiles.
- Whole notebook tab drag is disabled in BufferTabs::begin_drag: transferring one active route would strand other page contexts. Need grouped transfer payload for split/window rehome.
- Page reorder via up/down buttons; drag reorder pending. Notebook-wide search, graphical chooser, standalone-page view sharing, notes-sidebar notebook folder icons pending.
- A page already open in another notebook is rejected, not duplicated. Existing per-page buffers are reused.
- Dirty close guards require saving edited pages individually. Resume persists last page, not per-page caret/scroll across app restart. In-session states are preserved.
- File moves through Neoism update open notebook manifests; external renames aren't automatically repaired. External manifest edits cause refusal to overwrite; reopen to reload.

## Safety
Never call context activation/path lookup while holding a notebook session mutex (lookup can lock it). Remove group contexts by stable route IDs, resolving fresh Taffy node IDs after every removal because layout rebuild renumbers nodes.
