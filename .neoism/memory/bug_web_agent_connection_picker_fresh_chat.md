---
name: "Web fresh chats credential picker — FIXED"
description: "Provider-scoped connection preference survives web fresh tabs while session model/thinking/history remain isolated."
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-01"
updated: "2026-08-01"
---

# Web fresh chats repeatedly asked API vs OAuth — fixed

Root cause: web `TerminalPanel` allocates every Alt+A agent buffer through WASM `agent_new_thread`; `ConfigDefaults` carries model/agent/thinking but no `connection_id`. Shared `NeoismAgentPane` only had the active session's `connection_id`, so account reconciliation on a fresh/default-model draft had no provider-scoped choice and multiple connections reopened `ModelAccount`. Copying a cached session's whole ProviderState would leak session model/thinking/history.

Fix:
- Shared pane now owns `provider_connection_preferences: HashMap<provider, connection_id>` separately from session state.
- Explicit model/account choices and provider state seed the map; authoritative ProviderState still assigns the exact active session connection (including clearing it) while a prior provider preference survives.
- Account reconciliation checks provider preference before opening the picker.
- WASM `agent_new_thread` calls `start_new_conversation_with_defaults`, resetting model/agent/thinking from ConfigDefaults while restoring only the selected credential for that model's provider. Existing `create_agent_thread_with_defaults` reads `pane.connection_id()`, so CreateThread now receives the restored id.
- Session cache remains unchanged and authoritative per-session state/history stays isolated.

Files: shared `state.rs`, `state/caches.rs`, `state/connect.rs`, `state/session.rs`, `state/tests.rs`; wasm `rendered/agent.rs`.

Regression: `fresh_chats_reuse_provider_connection_without_leaking_session_defaults` runs two successive fresh chats, verifies OAuth id reuse/no picker, fresh config model+thinking, and old session cache integrity.

Checks: focused + account/authoritative tests pass; all 2197 neoism-ui lib tests pass; cargo check neoism-ui, native wasm crate, and wasm32 web target pass. `cargo fmt --check` only reports an unrelated pre-existing formatting diff in state/tests.rs around subagent setup after own lines were formatted.
