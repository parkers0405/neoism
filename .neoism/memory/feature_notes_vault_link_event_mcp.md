---
name: "feature_notes_vault_link_event_mcp"
description: "Host vault-link broadcast + Notes MCP vaults/link"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-21"
updated: "2026-09-21"
---

Joined guests re-list Alt+N immediately after a vault link. Protocol `RefreshHostWorkspaceNotes` re-resolves `linked_vault_dir` and broadcasts `HostWorkspaceUpserted` (originator) plus `HostWorkspaceTree` (everyone else). UI send after local link. Daemon watches `{root}/.neoism/workspace.json` so MCP/manual JSON writes also refresh. Desktop `maybe_refresh_served_notes_sidebar` re-points when viewing the shared vault or the no-vault empty state. Notes MCP: `vaults` (allow in plan) lists registered vaults; `link` (deny in plan) takes `vault` name, optional `notes_path` and `path`, uses `link_code_dir_to_workspace_vault` / `link_code_dir_to_notes_scope`. Do not hand-edit project.json / workspace.json. --- name: "notes vault link event + MCP" description: "joined guests re-list Alt+N on vault link; notes MCP vaults/link tools" type: "feature"
