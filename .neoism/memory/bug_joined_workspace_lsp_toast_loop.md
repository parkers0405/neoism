---
name: "Joined workspace LSP toast loop"
description: "Joined workspace LSP Cargo.lock probe and Taplo repeated toasts"
type: "bug"
scope: "project"
origin: "user screenshots and local code investigation"
created: "2026-09-25"
updated: "2026-09-25"
---

Joined desktop routes code LSP to owning host via `remote.rs`; caret-idle `DocumentHighlight` fires on each cursor move/buffer revision. `Cargo.lock` has no LSP extension/filename route, so host `native_queries::query_at` returned unsupported Error and guest toasted every reply (toast UI merges only 1.5s). `.toml` matches TOML adapter but `taplo` resolution is on HOST, via host's managed binary or PATH; missing host executable is a genuine status error. Fix 2026-08: daemon returns empty highlights for no-route files, guest suppresses automatic highlight error toasts and deduplicates passive Sync/Hover/Completion/SignatureHelp error messages per subscription/document while keeping explicit actions visible. In `remote.rs` `Subscription.reported_errors` is retained across revision/focus, dropped on document reopen. Regression tests in `editor_lsp_ws.rs` and remote unit tests. `cargo check -p neoism -p neoism-workspace-daemon` passed; general `cargo check --tests` hit unrelated preexisting inaccessible `todo_items_from_response` in dirty `neoism-workspace-daemon/src/agent/tests.rs`.
