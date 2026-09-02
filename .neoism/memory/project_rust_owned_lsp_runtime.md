---
name: "Rust-owned LSP runtime cutover"
description: "Rust-owned LSP runtime facade, diagnostics bridge, typed actions/results/navigation, modal surfaces with selectable location buttons, multi-file formatting/code-action/rename edit application, codeAction resolve/executeCommand, and Neovim ownership hook removals"
type: "project"
scope: "project"
origin: "session 2026-07-06"
created: "2026-07-06"
updated: "2026-07-06"
---

Rust-owned LSP runtime cutover 2026-07-06. `neoism-agent-server::rust_lsp` is the blessed facade over the persistent Rust LSP client. Server command resolution is centralized in `lsp::resolve_lsp_command`: explicit config path, Neoism Extensions managed-bin map, raw PATH, missing. `LspStatus` includes `command_source` and `Available`.

Workspace daemon Rust LSP bridge: `neoism-workspace-daemon/src/rust_lsp.rs` uses active buffer text/cursor from `NvimSessionHandle::read_active_buffer` only as document transport, then calls `neoism_agent_server::rust_lsp::touch_document` / `status`; diagnostics WebSocket polling tries Rust LSP first and falls back to old Neovim diagnostics only if Rust cannot produce one.

Typed actions/results/navigation: `EditorClientMessage::LspAction` + `EditorLspAction` carry context-menu and palette/modal LSP actions; LspAction includes optional `text` payload for rename/workspace-symbol queries. Desktop maps context-menu actions and rename/workspace-symbol modal submissions to this protocol instead of Lua. `BufferText` includes cursor line/col. `run_action` calls real Rust hover/definition/references/implementation/document_symbols and returns `EditorServerMessage::LspActionResult` with summary, hover, locations, symbol_count. `EditorClientMessage::OpenBuffer` carries optional line/character; daemon auto-opens first definition/implementation location via typed OpenBuffer.

Desktop result surfaces: `Context` stores `editor_lsp_action_result` and modal seen flag. References push an info notification preview up to five locations. `Screen::maybe_open_lsp_action_result_modal` opens modal for Hover/References/DocumentSymbols. LSP location modals now include `Open 1..5` buttons via `ModalAction::OpenLspLocation`; selecting one sends typed `OpenBuffer { path, line, character }` through `EditorBackend::open_buffer_at_location`. This gives references modal selectable navigation for the first five locations without Lua.

Formatting/edit application: `rust_lsp::formatting` exposes Rust LSP formatting edits. Workspace daemon `run_action(Format)` applies formatting edits to active buffer text in Rust and pushes via `NvimSessionHandle::apply_authoritative_text`. `apply_lsp_text_edits` applies edits bottom-up, rejects overlaps, validates ranges/UTF-8 boundaries.

Code actions: Rust supports `code_actions`, `codeAction/resolve`, and `workspace/executeCommand`. Workspace daemon tries direct edit, resolved edit, then command-only execution. Direct/resolved edits apply through grouped multi-file workspace edits.

Rename: Rust supports `textDocument/rename`; desktop rename modal sends typed rename with text; daemon applies multi-file rename workspace edits.

Workspace edits: `workspace_edits` extracts `edit.changes` and text `edit.documentChanges` into `BTreeMap<PathBuf, Vec<TextEditJson>>`, rejects resource operations/unsupported URIs. `apply_workspace_edits` applies active-file edits through `apply_authoritative_text`; other files are read from disk, safely edited, and written back.

Removed Neovim LSP ownership hooks: deleted `vim_lsp_action_command`; removed `refresh_managed_bin_map_in_nvim` and startup managed-bin push from desktop; deleted `vim_set_managed_bin_map_command`. Rust extension-managed LSP resolution is source of truth. Remaining direct `rio.lsp` strings are old fallback/status/Lua runtime and shared modal policy strings desktop now bypasses.

Checks/tests passed: `cargo test -p neoism-workspace-daemon rust_lsp --lib`; `cargo check -p neoism-protocol`, `neoism-agent-server`, `neoism-backend`, `neoism-workspace-daemon`, `neoism`. Remaining for full golden-standard Zed-like LSP: richer full references/symbols picker (beyond first five modal buttons), direct editor document model sync instead of Neovim text read, and eventual delete/demote `rio/lsp.lua`.
