---
name: project-unified-config
description: "config.json is THE config (terminal + agent in one file, JSONC, hot reload); agent/ subfolder gone; workspaces.toml deleted — dialect + collision gotchas (2026-07-14)"
metadata: 
  node_type: memory
  type: project
  originSessionId: b38427bd-f724-4a0c-a271-2f226f3736c2
---

**Unified config (2026-07-14, uncommitted):** `~/.config/neoism/config.json` is the primary config for BOTH the terminal app and the agent server. No more `agent/` subfolder. Legacy `config.toml` still honored when no config.json exists (`config_file_path()` prefers json).

**Dialect:** JSONC — `//`, `/* */`, trailing commas. Strippers exist in THREE places (keep in sync): agent-server `config_parse.rs` (original), `neoism-backend/config/mod.rs` (`strip_json_comments`/`strip_trailing_commas` + `parse_config_content` extension dispatch), `neoism-extensions/agent_config.rs`. Programmatic writes (`write_config_section` → theme picker/mashup/minimap/fonts; extensions MCP installs) emit pretty JSON and **lose hand-written comments** — accepted trade.

**Merged-file collision:** the only key both sides define is `shell` (app: `{program, args}` table; agent: string). Fixed via lenient `deserialize_shell` in neoism-agent-core api.rs (string OR table → "program args" string). Everything else: agent `NeoismConfig` has `#[serde(flatten)] extra` catch-all; app serde ignores unknown keys. Watch for NEW top-level keys colliding in future.

**Paths:** agent `default_config_dir()` (server_util.rs) = `~/.config/neoism` now; `NEOISM_AGENT_CONFIG_DIR` env overrides (used by skill tests for hermetic isolation — real skills in `~/.config/neoism/skills` leaked into `skill::tests` counts otherwise). Skills: `~/.config/neoism/skills/`. Markdown agent defs: `~/.config/neoism/agent/<name>.md` (merge_markdown_entries scans agent/agents/mode/command subdirs of config dir). extensions `agent_config_path()` = `neoism/config.json`. Hot reload: watcher accepts config.json + config.toml filenames; agent side re-reads per request (no watcher needed).

**Parker's migration done:** config.toml + agent/config.json merged → config.json; `agent/skills` → `skills/`; legacy files fully deleted (user asked for clean cutover — old binaries see defaults until rebuilt). Default first-run template (`default_config_file_content`) is now JSONC.

**Skip permissions (same day):** config key `dangerouslySkipPermissions` (aliases: snake + kebab; agent-core NeoismConfig) → normalize_config injects `"*": "allow"` into the permission map (BTreeMap puts "*" FIRST; evaluate is last-match-wins so explicit denies still deny). Parker's config has it ON (re-enabled 2026-07-14 after first reverting). Session-scoped: `/yolo` | `/dangerously-skip-permissions` | `/skip-permissions` → `SlashCommandAction::ToggleSkipPermissions` → pane `skip_permissions` flag auto-answers "Yes" (Once) on enqueue + on queue promotion (`maybe_auto_respond_permission` in enqueue_pending_permission + permission_reply_succeeded, both panes).

**Standalone mcp.json (same day):** `~/.config/neoism/mcp.json` = MCP catalog file (like skills get their own home) — agent loader `merge_mcp_file` merges it per config dir AFTER config files (wrapped `{"mcp":{...}}` or bare map both accepted; entries win over config.json). Extensions page installs write there (`mcp_config_path()`), uninstall/disable clean BOTH files, install drops same-id from config.json to prevent deep-merge blending. Parker's 12 servers moved to mcp.json.

**Also that day:** `workspaces.toml` machinery DELETED (was write-only — real registry is daemon's `~/.local/share/neoism/workspaces.json`); status-bar `tab_position` renamed `cursor_lines` = nvim-style ruler (nvim WinbarNotification / markdown pane cursor_line; hidden on terminals).

**Pre-existing failures seen (NOT mine):** agent-server `tests::compacted_summary_trims...` (other session's stream work), `examples/completion_probe.rs` doesn't compile (intentional fixture? blocks `cargo test -p neoism-agent-server` without `--lib`), workspace-index `note_graph_queries...`.
