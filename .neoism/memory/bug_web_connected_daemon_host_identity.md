---
name: "Web connected-daemon stable host identity — FIXED"
description: "HelloAck stable connected_host_id replaces URL alias host classification in web Notes/modal"
type: "bug"
scope: "project"
origin: "current-session"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented durable connected-daemon identity for web Notes/workspace classification. `WorkspaceServerMessage::HelloAck` now has backward-compatible `connected_host_id: Option<String>` populated with `machine_host_id()` only on accepted handshakes. Web `ProtocolClient` caches it before callbacks; `WorkplaceService` caches active identity, clears on disconnect, and ignores stale HelloAck from superseded clients. `App` no longer compares URL host:port: shared `app/workspaceHostIdentity.ts` classifies by stable host IDs. Missing ID from old daemons conservatively treats entries from the connected tree as own. Own Notes uses `notes_vault_dir` (linked fallback); proven foreign uses only `linked_vault_dir`. Workspace modal local/remote labels use the same helper. Tests cover Rust old serde payload + accepted ack ID, TS alias/same URL identity cases, fallback, vault choice, ProtocolClient cache, Workplace stale-ack race. Verified cargo check daemon, protocol HelloAck tests, daemon unit/integration handshake tests, web typecheck + all 134 tests, changed Rust rustfmt check, git diff --check.
