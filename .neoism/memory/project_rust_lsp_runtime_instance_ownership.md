---
name: "Rust LSP runtime instance ownership"
description: "Explicit per-instance Rust LSP runtime ownership and deterministic teardown"
type: "project"
scope: "project"
origin: "neoism_agent_v2 implementation session"
created: "2026-08-25"
updated: "2026-08-25"
---

As of branch `neoism_agent_v2`, the Rust LSP engine uses explicit `LspRuntime` instance ownership. Each server/daemon `AppState` owns one runtime, and the runtime owns immutable injected `AgentServices`, clients, diagnostics broadcaster/cache, adapter cache, and cargo metadata/root cache. No mutable backend LSP statics remain; immutable built-in language metadata remains global. Client diagnostic callbacks hold `Weak<LspService>` to avoid ownership cycles. Daemon live-document workers include `runtime.instance_key()` in their keys so two runtimes cannot share queues. Deterministic shutdown sends `shutdown`, waits boundedly, sends `exit`, briefly allows graceful process termination, then Drop kills the process group and waits if necessary. Isolation tests cover diagnostics events plus all mutable caches/clients; stdio e2e asserts both shutdown and exit. Required LSP tests pass (80 passed, 5 ignored), as do server, CLI, daemon, and router/OpenAPI checks. Desktop LSP call sites compile through their modules, but the full desktop crate is currently blocked by unrelated concurrent errors in `neoism/agent/pane.rs` (missing Arc import) and `host/finder_search.rs` (missing FffWorkspaceSearchService::pin).
