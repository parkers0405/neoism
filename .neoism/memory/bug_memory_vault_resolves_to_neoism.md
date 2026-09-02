---
name: bug_memory_vault_resolves_to_neoism
description: "Agent project-memory wrote to the Neoism vault from unrelated dirs (home) — project_root fell back to load_workspace reading ~/.neoism/workspace.toml (workspace=\"Neoism\"); FIXED to linked-vault-else-Default"
metadata: 
  node_type: memory
  type: project
  originSessionId: 7f62545c-19db-444c-86e4-3b6a6574bcc7
  modified: 2026-08-19T19:28:52.942Z
---

MCP neoism-memory `memory.write` scope:project kept writing to
`~/Neoism/Vaults/Neoism/Memory` for UNRELATED work (e.g.
feature_minecraft_auction_bounty_gui.md) even though the agent wasn't in the
neoism source. User's rule (stated many times): if the working dir has NO
LINKED vault, use the DEFAULT vault's Memory — never the Neoism vault.

Root cause: `mcp_memory.rs::project_root(cwd)` resolved
`linked_project_for_code_dir(cwd)` (vault project.toml [[links]] — the real
"link") .or_else(load_workspace(cwd)) .unwrap_or(Default). The Neoism vault's
only link is `/home/parkersettle/projects/neoism`, so home/Minecraft dirs don't
match step 1. BUT most agent sessions run with dir=`/home/parkersettle`, and
`~/.neoism/workspace.toml` declares `[notes] workspace = "Neoism"` — so step 2
`load_workspace(cwd)` returned the "Neoism" workspace → Neoism vault. A
dir-local `.neoism/workspace.toml` is NOT a vault link.

Fix (mcp_memory.rs project_root, cargo-check clean, UNCOMMITTED): drop the
`load_workspace` middle step → `linked_project_for_code_dir(cwd)` else
`WorkspaceConfig::new(cwd)` (whose notes.workspace defaults to "Default"). So:
genuinely-linked dir → that vault; anything else → Default/Memory. Covers
write + read/list/recall (all route through project_root via roots_for_scope).
NOTES tool had the SAME bug (2026-07-30, user reported it live: "Notes
overview → active notes vault is .../Vaults/Neoism" while in home root). Both
notes entry points funneled through `NoteGraph::open(cwd)` (query.rs), whose
middle fallback is `load_workspace(root)` → same home→Neoism path. FIXED
scoped to the agent (NOT the shared NoteGraph::open, which desktop callers —
main.rs/vault_ops.rs/tags_view.rs — legitimately use with a real opened root):
- mcp_notes.rs: new `pub(crate) resolve_notes_graph(cwd)` = linked-else-
  `default_vault_graph()`; `default_vault_graph()` builds from the blessed
  `neoism_workspace_index::default_notes_workspace()` (stable id, notes
  enabled). "auto"→resolve_notes_graph, "vault"→default_vault_graph,
  all_vault_graphs now uses `config::vault_notes_workspace(name)` per vault
  (dropped the load_workspace base + cwd param).
- tool_support/notes.rs (built-in `notes` tool): replaced graph_root+
  `NoteGraph::open` with `crate::mcp_notes::resolve_notes_graph(&context.cwd)`.
cargo-check clean, UNCOMMITTED, no build/push.
Latent footgun found: `~/projects/neoism/.neoism/workspace.toml` is git-TRACKED
(says workspace="Default"). Data note: `~/.neoism/workspace.toml` workspace=
"Neoism" is itself questionable (home ≠ Neoism vault) but left alone (user's
config; code fix makes it irrelevant for memory).

2026-08-19 READ-SIDE follow-up (write side above was correct all along). User
hit `Read(Memory/MEMORY.md) error: failed to resolve path
/home/parkersettle/Github/synapse-ai-hub/Memory/MEMORY.md`. Memory lives in the
VAULT, outside the workspace, but nothing ever told the model that: the injected
index header said only "vault X" with no absolute path, and agent_native.rs's
prompt said memory "lives in the working directory's linked vault `Memory/`
folder" — which reads as cwd-relative. Fixed in mcp_memory.rs +
tool_support/paths.rs:
- injected header now carries the ABSOLUTE memory folder + "OUTSIDE the
  workspace, never read it with a workspace-relative path".
- `existing_project_path` falls back to `memory_file_for_workspace_path` when a
  relative path fails to canonicalize and starts with `Memory/` (or is
  `MEMORY.md`) → the read lands in the vault instead of erroring.
- `memory.read` searches EVERY root in scope (was `roots.first()` only, so
  user-scope files failed under default auto scope), strips a re-typed
  `Memory/` or `Memory/Personal/` prefix, and accepts the `absolutePath` it
  reported earlier.
- injected index truncation was mid-line at 8k chars with no marker (Neoism's
  own index is ~17.8k → half silently invisible): now 12k, cut on line
  boundaries, ends with "(index truncated - call memory.list or
  memory.recall...)".
- v0.7.46 vault-FOLDER links (project.json `notes_path`) scope project memory to
  `<vault>/<folder>/Memory`; reads now also cover `<vault>/Memory` so
  pre-link/shared memory stays reachable, writes still go only to the scoped
  folder. 6 mcp_memory tests, full 427-test suite green.
