---
name: "Notes notebook menus and MCP tools"
description: "Notes right-click New Notebook + conversion/open; desktop notes MCP gains five notebook tools using shared creation, vault confinement, and plan permissions."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-16"
updated: "2026-09-16"
---

# Notes sidebar notebook entry points and agent MCP

Follow-up to feature_documentation_notebooks.md implemented in the same dirty workspace; no commit/release build.

## Notes UI
- desktop screen/bridges/workspace/notes_menus.rs adds New Notebook to the Notes right-click menu (including right-click on Add/header), plus Turn Folder into Notebook/Open Notebook on directories. Remote joined-workspace notebook actions disabled (notebook hosting still local-only).
- New ModalAction::NotesPromptNewNotebook {dir} opens the existing documentation notebook creation modal with captured parent dir. `open_documentation_notebook_prompt_at` roots creation at the viewed vault/selected folder, not terminal cwd or active notebook.
- sidebar.rs opens notebook directories on click or Enter; horizontal tree expansion still works. Existing one-click New Note/Add behavior retained.

## Shared creation
- `neoism-ui::editor::documentation_notebook::NotebookBinding::create(folder,title)` in new documentation_notebook/storage.rs centralizes bounded import, starter overview, optional title, nested sections, and no-overwrite create_new semantics. Desktop and MCP both call this. Old desktop-local collector removed.

## Notes MCP
- Actual current notes provider is `neoism-frontend/desktop/src/notes_mcp.rs`, a stdio process (`neoism --neoism-notes-mcp`) injected through DesktopNotesConfig. Older memory about built-in adapter notes is outdated for this surface. Generic agent server remains product-agnostic.
- New child notes_mcp/notebooks.rs defines and dispatches notebookList, notebookCreate, notebookRead, notebookAddPage, notebookMovePage. Runtime names mcp__notes__<name>.
- All paths relative to workspace's linked/default Notes vault. AddPage accepts EITHER title+optional content OR existing_path relative to vault. Existing pages may be elsewhere within vault; outside-vault writes/links through this MCP are not permitted. checked_path canonicalizes existing ancestors and manifest itself to block symlink escapes.
- NotebookCreate accepts folder path, optional title, imports existing MD or seeds overview. NotebookRead returns JSON {path,title,pages}. MovePage uses one-based page + direction up/down. Files never overwritten by create/add. New page preserved/reported if subsequent manifest persistence fails.
- Plan permissions: notebookList/notebookRead allow; notebookCreate/AddPage/MovePage deny.
- Unit coverage: create/read/reorder/reference, invalid args, traversal, duplicate no-overwrite, symlink rejection, shared creator empty/import, plan permission assertions. Existing desktop/tests/notes_mcp_agent.rs expanded tool discovery and HTTP->MCP notebook create/add/read integration coverage.
- Guide docs/documentation-notebooks.md updated. Running MCP process needs the next rebuilt/restarted desktop to advertise new tools; no claim of hot-updating the current binary.
- Checks use cargo check --tests (not executed tests) for neoism-ui and neoism; wasm compatibility check as well. No release build.
