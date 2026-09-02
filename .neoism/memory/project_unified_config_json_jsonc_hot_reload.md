---
name: "Unified config.json (JSONC, hot reload)"
description: "Unified config now has schema-driven native completion and Settings, host-aware suggestions, Alt+, direct opening, JSONC-preserving writes, and remote config documents."
type: "project"
scope: "project"
origin: "Implemented config discoverability and direct config editor"
created: "2026-08-20"
updated: "2026-08-03"
---

## Config intelligence and Settings

Implemented 2026-08-03.

- `Alt+,` is the cross-platform primary binding for `ConfigEditor`; it now means **Open Neoism Config**, not GUI Settings or external editor. Desktop keyboard, palette, and Settings file button ensure `~/.config/neoism/config.json` exists and open/focus it in the native editor without changing workspace root/CWD. Web opens the connected daemon host config as `neoism://host-config/config.json`.
- GUI Settings is a separate `PaletteAction::OpenSettings` and command-palette entry. Both GUI and raw JSONC operate on the same canonical config.
- Canonical metadata uses `neoism_protocol::config::ConfigDescriptor`; backend `config::intelligence::config_descriptors()` supplies curated rows plus automatic leaves from serialized backend `Config`, platform override templates, serialized `neoism_agent_core::api::NeoismConfig`, and wildcard custom-agent/mode fields (`agent.agent.*.*`).
- Runtime suggestions include installed fonts/themes/shells/custom agents; daemon adds live workspace LSP adapter IDs. Extensible choices remain editable.
- `neoism-protocol/src/config_intelligence.rs` provides host-neutral nested JSONC key/value completion and descriptor hover, including comments, kebab keys, object snippets, and wildcard map paths. Desktop intercepts the canonical local config in its code LSP worker; WASM/shared handles the host-config virtual path.
- Settings no longer has a hardcoded SETTINGS catalog. `NeoismSettingsPane` owns descriptor-derived rows, maps all categories/controls, merges runtime/static choices, keeps specialized font/theme/model/keybind controls, and provides typed string/number/array/object editing. Desktop and web seed it from the same schema.
- Protocol config plane supports `GetConfigSchema`, `GetConfigDocument`, `EnsureConfigDocument`, and revision-checked `SaveConfigDocument`; daemon owns path resolution and permissions. Web excludes virtual config from workspace CRDT binding and saves through this protocol.
- Backend config creation is create-if-absent `Result<PathBuf>`; complete document saves validate app and agent JSONC and atomically replace with revision conflict checks. GUI `write_setting`/`write_keybind` use targeted JSONC edits preserving comments, trailing commas, ordering, and unrelated formatting.
- Docs updated in Configure Neoism, Essential Keybindings, Navigation and Keybindings, starter config comments, and README.

Verification: protocol config-intelligence tests pass; backend descriptor/raw-document/JSONC-edit tests pass; Settings tests pass; 72 command-palette tests pass; `cargo check` passes for protocol, agent-core, backend, ui, and wasm; web TypeScript check passes. Full desktop/daemon check is independently blocked while `neoism-agent-server/src/workflow.rs` is absent in concurrent work.
