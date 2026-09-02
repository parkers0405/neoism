---
name: project_notes_sidebar
description: "Notes sidebar (Alt+N) hierarchy, icon assignment, creation targeting — invariants and gotchas"
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

Notes sidebar (`shared/src/panels/notes_sidebar.rs`, desktop glue in
`desktop/src/screen/bridges/workspace.rs`).

- **Vertical hierarchy (user-specified)**: header icons (share `f1e0` + ⋮ menu
  `f142`, top right) → note rows → vault selector (footer, full width). Up from
  first row → header icons; Down from icons → first row (or selector when
  empty); Left/Right toggle share↔menu. Reaching icons via ArrowRight from the
  selector was explicitly rejected.
- **Creation always targets the VIEWED vault** (`notes_sidebar.workspace_path()`
  via `notes_creation_dir()`), never `active_pane_workspace_root` — that was the
  "creates in first-opened folder" bug. All creation lives behind the ⋮ menu
  (New Note/Drawing/Folder via ModalActions; `NotesNewDrawing` added).
- Icon assignment: click a row's icon → context-menu picker (emoji grid +
  Custom prompt + Reset) → `.neoism-icons.json` in vault root (rel path →
  glyph); loaded in `refresh_notes` (desktop fs only; web shows defaults).
- **Focus-chain gotchas**: `handle_buffer_tab_focus_key` (panes.rs) runs BEFORE
  the notes key handler and used to swallow Alt+Up/Down;
  `focus_buffer_tabs_for_current_pane` must unfocus notes_sidebar too, and the
  `Alt+Right → git panel when visible` shortcut must be guarded by
  `!file_tree/notes focused` or it creates stuck double-focus.
- `set_focused(false)` clears `header_action`. ModalAction additions need: enum
  (modal.rs), `with_input` arm if it takes input, `ModalActionTag` +
  `modal_action_dispatch` (chrome_policy.rs), tag mapping + executor arm
  (desktop lifecycle.rs). `ModalSpec` requires `busy`/`blocking` fields.
- Phantom build errors in `neoism-agent-server` (missing `GoalResearchNote`
  etc. while the source clearly has them) = stale cargo fingerprints from
  another agent's mid-flight commits: `cargo clean -p neoism-agent-server -p
  neoism-agent-core` fixes it.
