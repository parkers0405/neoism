---
name: "Desktop-owned Notes MCP"
description: "Notes removed from generic Agent and supplied automatically by Neoism Desktop over stdio MCP; memory remains native Agent-owned"
type: "feature"
scope: "project"
origin: "implementation"
created: "2026-08-28"
updated: "2026-08-28"
---

---
name: Desktop-owned Notes MCP
description: Notes removed from generic Agent and supplied automatically by Neoism Desktop over stdio MCP; memory remains native Agent-owned
 type: feature
---

Neoism Notes is owned by `neoism-frontend/desktop`, not generic `neoism-agent`. Desktop decorates Agent services with a final read-only runtime config layer named `neoism:desktop-notes-mcp`, registering server `notes` as a local stdio child of the current Neoism executable (`--neoism-notes-mcp`). The child runs with the declared Agent workspace as cwd, resolves the linked project notes folder, and falls back to the default vault.

Surface: `notes.list`, `notes.search`, `notes.read`, `notes.create`, `notes.write`, `notes.tasks`, `notes.taskToggle`; runtime IDs are `mcp__notes__*`. Plan may list/search/read/tasks but cannot create/write/toggle. Legacy request aliases remain accepted (`note` for `path`; `query` for create `title`) while schemas advertise canonical arguments. Tool failures return MCP `isError: true`; unknown RPC methods use JSON-RPC `-32601`.

Standalone/hosted Agent receives no Notes unless its host supplies an MCP. Agent memory remains the native `memory` tool, with project data at `<declared-workspace>/.neoism/memory` and personal data in Neoism's platform data directory. Old Notes-vault memory is physically migrated out.

Integration coverage: `neoism-frontend/desktop/tests/notes_mcp_agent.rs` launches the actual desktop binary's hidden stdio mode through the real Agent router, verifies all seven runtime IDs, and calls create/write/read plus an `isError` failure. Canonical gateway path casing includes `notes.taskToggle`.
