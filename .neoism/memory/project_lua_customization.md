---
name: "Neoism Lua customization platform"
description: "Neovim-style Lua runtime, isolated plugins, managed lifecycle, relocatable chrome, unified sidebar, and owner/revision-safe local/remote structured LSP reads and edits"
type: "project"
scope: "project"
origin: "agent implementation"
created: "2026-10-05"
updated: "2026-10-06"
---

# Lua customization platform

The native Lua customization runtime is complete: vendored Lua 5.4, immutable language-neutral snapshots, hierarchical styles, commands, mode-aware keymaps, autocmds, dynamic panels, declarative UI, typed object namespaces, sandboxing, memory/time budgets, authoritative mutations, and no-init zero-VM behavior.

Plugin ecosystem includes deterministic local/spec discovery, dependency graphs, isolated VM per plugin, plugin-root `lua/` module containment, owner/revision callback IDs, candidate generations, eager and lazy activation, immutable combined snapshots, and transactional whole-generation reload. Invalid config, plugin, or surface candidates preserve the prior config/runtime/manager/snapshot generation.

Lazy activation is wired for commands, events, exact-once key replay with mode/when metadata, authoritative editor filetypes, and active surfaces. Declarative specs, direct local packages, and immutable managed Git lockfile revisions merge into one validated graph. Git source/ref mismatches and package ID mismatches reject candidates.

Persistent grouped `plugins` config includes disabled IDs, per-plugin capability grants, update policy, and trusted Git source prefixes. `ScopedPluginHost` intersects declared capabilities with grants and enforces read/write/LSP access before query/dispatch. Host actions carry immutable plugin owner/revision metadata. Plugin loaded/lazy/failed/disabled state is published through the Lua host as `plugins` and emits `PluginsChanged`.

Relocatable chrome shipped as a generic data-only dock solver. Lua can call `neoism.ui.surface("chrome.actions", { dock = "left", thickness = 44 })` to replace the top action band with a VS Code-style side rail. Nine Rust-owned controls are independently hideable/reorderable/aligned through `neoism.ui.item`. One resolved layout drives reservation, paint, hitboxes, menu/tooltip direction, Island placement, sidebar/status positions, terminal/editor reflow, desktop, shared UI, and WASM. Unknown IDs and invalid dimensions reject transactional reloads. No Lua executes in layout/render/hit-test paths.

Extensions lifecycle is integrated into the normal Extensions page, not a Lua-specific tab. The default `All` view contains normal packages and Lua package rows. A tolerant inventory preserves row identity for discovered, lazy, loaded, disabled, permission-required, incompatible, blocked, failed, update, restore, installing, updating, restoring, and removing states without weakening transactional activation. Rows expose direct install/update/remove/restore/enable/disable/grant/revoke/retry actions.

Managed Lua acquisition derives version, manifest checksum, and dependencies from the validated checkout instead of trusting pre-checkout caller placeholders. Install/update/restore/remove run through one application-owned serialized background queue, publish stage progress, keep indeterminate animation alive, catch worker panics, retain retryable failures, and block removal when enabled dependents exist. Package validation failures preserve the old lock/revision. If package acquisition succeeds but the full manager candidate rejects a cross-plugin conflict, the store restores the exact prior lock entry before retaining the last-known-good runtime. Enable/disable and grant/revoke build a candidate first, then persist the complete grouped policy map/list and atomically swap manager state.

## Structured LSP

Compatibility calls such as `neoism.lsp.definition()`, `neoism.lsp.hover()`, `neoism.lsp.code_actions()`, `neoism.lsp.rename()`, and `neoism.lsp.format()` retain native UI behavior. `neoism.lsp.request(operation, args)` is the separate structured async API. Public portable types live in `neoism-lua/src/types.rs`; coordinates are zero-based lines and zero-based UTF-8 byte columns.

Desktop local and remote structured reads support hover, signature help, definition, references, document symbols, workspace symbols, diagnostics, and clients. Local requests use a dedicated non-coalescing FIFO worker sharing the process LSP runtime. Remote requests use correlated `LspRead`/`LspReadResult`; host paths stay opaque strings and guest validation uses `HostPath`, preserving Windows and UNC spelling.

Application pending state is keyed by exact `PluginOwner` plus Lua invocation ID and stores window, operation, and immutable target. Results are checked against all captured metadata and delivered only to that owner. Reloaded/stale owners cannot receive old results. Logical cancellation removes exact owner-scoped pending state, emits one terminal cancellation event, suppresses late results, and removes remote transport state; full JSON-RPC `$/cancelRequest` remains future work. Limits are 64 pending requests per owner and 512 process-wide.

Structured edits are complete for `code_actions`, `apply_code_action`, `rename`, and `format`. Code-action results expose only portable summaries. Selection uses an application-owned, random, one-shot, five-minute lease keyed by exact owner, source request, and action ID; Lua cannot override the retained root, path, or position. Raw server IDs, commands, action payloads, workspace edits, digests, and plans remain Rust-private. Desktop action limits are 256 per owner and 2048 process-wide.

Local workers only prepare typed mutations. Application-thread commit revalidates active owner, exact pending target, open-buffer revisions, closed-file digests, workspace and symlink containment, ownership transitions, every UTF-8 boundary/range, duplicate starts, and overlap before mutation. Open files use `CodeBuffer::apply_text_edits` followed by normal modified/CRDT synchronization; closed files are written only after whole-plan preflight. Deferred commands run only after edits commit and updated open documents are synchronized.

Remote edits use `LspEditPrepare`, `LspEditCommit`, and `LspEditFinalize`. The daemon keeps connection-local random one-shot action, plan, and command capabilities with five-minute expiry and bounded vaults (2048 actions, 512 plans, 512 commands). Prepare validates and snapshots open revisions and closed digests without mutation. The application revalidates owner/target before authorizing commit. Commit consumes the plan, rechecks exact open revisions, rejects files that became open, rechecks closed digests, patches only closed host files, and returns typed edits for frontend-owned buffers. The frontend completely preflights the response, applies and synchronizes open edits, then finalize consumes and executes any private deferred command. Cross-socket capability use and all replay attempts fail.

Structured format is open-buffer-only locally, in remote desktop dispatch, and at daemon prepare. It applies through the owning frontend, leaves the buffer dirty, and never saves, marks saved, queues `SaveBuffer`, or emits `DidSave`. The real stdio fixture asserts no `textDocument/didSave` call.

Resource operations remain unsupported. Malformed edits, unsupported URIs, outside-root targets, symlink escapes, invalid UTF-8 boundaries, reversed/out-of-range ranges, overlaps, duplicate-start inserts, stale revisions, changed digests, changed ownership, and mismatched response paths/edit counts abort before frontend mutation. WASM explicitly ignores desktop-only correlated structured edit results. `docs/lua.md` documents arguments, portable outcomes, capability semantics, preflight, ownership, and no-save formatting.

## Unified left sidebar

Files, Notes, and Conversations default to one Rust-owned VS Code-style left slot through shared `LeftSidebarHost`. The controller owns active unified view, focus, one 300px resizable unified width, and per-view `Unified`/`Independent` placement; panel objects retain their own selection, scrolling, sessions, composer/chat state, and natural independent widths. Mixed order is active unified slot, independent Files, independent Notes, independent Conversations, then main content.

Configuration is grouped under `ui.left-sidebar.file-tree`, `ui.left-sidebar.notes`, and `ui.left-sidebar.conversations`, each accepting `"unified"` or `"independent"` and defaulting to unified. Desktop and WASM apply these settings at construction and hot reload. Desktop workspace state persists the active unified view, while geometry, painting, text occlusion, hit-testing, wheel input, resizing, and spatial focus all consume the same resolved view sequence. Lua/native panel visibility actions route through the host.

Both top agent controls emit `ToggleConversations`; `OpenAgent` remains only for explicit chat creation flows. Toggling Conversations never calls desktop `open_neoism_agent_tab` or the WASM new-tab route, and regression tests preserve active tab count/index/route across open-close.

Verification baseline after structured edits: `cargo check -p neoism-protocol -p neoism-workspace-daemon -p neoism -p neoism-terminal-wasm` passes; protocol editor tests pass 28; strict shared edit tests pass 2; Lua runtime tests pass 12; focused desktop owner-scoped cancellation passes; and the real daemon websocket/stdin fixture passes read/edit preparation, socket isolation, action/plan/command replay rejection, open revision invalidation, closed digest invalidation, open-only format, no-save behavior, and deferred command ordering. Final touched-file `git diff --check` passes. Existing unrelated warnings remain. Do not mass-format the dirty repository.
