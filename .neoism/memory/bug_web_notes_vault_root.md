---
name: bug-web-notes-vault-root
description: Web notes sidebar listed <workspace_root>/notes instead of the linked vault — FIXED 2026-08-19
metadata:
  type: project
---

Web/Chrome Alt+N notes listed `<workspace_root>/notes`, a directory the vault
model never writes. Desktop, the daemon's own note-create (`workspace/shell_ops.rs`)
and the agent notes tools all resolve `linked_project_for_code_dir(root) →
notes_workspace_dir()` = `~/Neoism/Vaults/...`. Tell: web CREATED notes into the
vault but LISTED `<root>/notes`, so a web-made note could never appear.

Second defect stacked on it: `NotesSidebar::set_entries_from_host` expects
daemon-ABSOLUTE paths and derives depth by `strip_prefix(root)`; web pushed
RELATIVE `notes/...`, so even a correct listing rendered flat.

**Why:** the vault was already on the wire as `WorkspaceSummary::linked_vault_dir`,
but the TS mirror dropped the field (zero `linked_vault` hits in web/ or wasm/).

**How to apply:** Chrome takes `notes_vault_root` from the host — never derive a
notes dir from the workspace root. Two wire fields now exist and mean different
things: `linked_vault_dir` = explicit link only (what a GUEST must use, `None` →
"no linked vault" empty state, so a guest never sees the host's personal vault);
`notes_vault_dir` = desktop's full chain `linked → filter(notes.enabled) →
default_notes_workspace()`, NOT gated on the dir existing, used when viewing your
OWN host (`App.ts:isOwnHostWorkspace`). Any file request for a note must route
through `TerminalPanel.filesRootForPath()` — vault paths sit OUTSIDE the workspace
root and the daemon rejects an absolute path that escapes its given root.

Related: [[project-notes-sidebar]], [[project-shared-notes-agent-scoping]],
[[project-notes-overhaul]], [[feedback-desktop-vs-web-paths]]
