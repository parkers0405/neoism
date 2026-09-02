---
name: "Agent V2 plugin-first migration"
description: "Agent V2 plugin runtime migration complete; generation-safe execution, LKG config, authenticated binding, wait/CLI correlation shipped in 0a954d31a"
type: "project"
scope: "project"
origin: "session completion 2026-08-25"
created: "2026-08-25"
updated: "2026-08-25"
---

Agent V2 plugin-first migration completed and pushed on branch `neoism_agent_v2` in commit `0a954d31aa2ab7c50946b07442b8a254122bcfe5`.

Key invariants shipped:
- A selected `PluginGenerationLease` owns registry snapshot, parsed config, lifecycle resources, services, and canonical workspace root.
- Turns, tools, routes, WebSockets, MCP callbacks, background jobs, subagents, workflow, semantic, LSP, and PTY execution retain the selected generation rather than reacquiring current state.
- Operational task-local generation lookup is separate from control-plane `published_snapshot()` refresh/reconciliation.
- Reconciliation is freshness-checked and serialized so stale generations cannot overwrite global workflow or semantic state.
- Invalid config reloads preserve the last-known-good generation.
- Skills use path-derived IDs distinct from display names and resolve content at invocation from the captured source.
- Authenticated workspaces use opaque daemon UUID strings; `/wait` is queue-worker race-safe; CLI responses correlate by assistant `parent_id` to a stable submitted message ID.
- Canonical plugin manifest route is `/v2/plugins/{plugin_id}/manifest`.

Verification:
- Strict warning-denied checks passed for plugin-api, builtins, server, and CLI tests.
- Architecture guards: 8/8 passed without raising ceilings.
- Plugin API tests: passed.
- Builtins: 69/69 passed.
- Server: 412 tests passed when run single-threaded; 5 environment-dependent LSP tests ignored. Parallel runs can interfere with the LSP restart subprocess test, which also passed in isolation.
- CLI: 38/38 passed.
- TypeScript contract generator tests passed.
- `git diff --check` passed.
- `cargo fmt --all --check` is not a useful migration gate because the repository currently has 153 pre-existing unformatted Rust files; formatting-only churn was intentionally excluded.
