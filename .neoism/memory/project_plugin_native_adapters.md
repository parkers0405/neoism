---
name: "Native IDE plugin adapters E4–E10"
description: "Exact-owner plugin declarations plus native completion, syntax, task/test, PTY, DAP, Git, and Agent read adapters; trust blockers."
type: "project"
scope: "project"
origin: "coding session"
created: "2026-08-01"
updated: "2026-08-01"
---

# Native IDE plugin adapters (E4–E10)

Implemented portable exact-owner platform declarations and native desktop adapters across `neoism-lua` and `neoism-frontend`.

## Shipped adapters
- Completion candidates install into the Rust-owned completion menu with exact document revision/cursor checks, max 256, native snippet insertion, and CRDT synchronization. Explicit `completion.resolve` invokes only the exact-owner command.
- Built-in Tree-sitter query execution runs on worker threads against authoritative exact-revision snapshots; no dynamic parser loading.
- Task/test/watch execution uses bounded `LuaJobs` with output streaming, timeout, cancellation, process-tree teardown, and owner retirement.
- PTY broker in `desktop/src/lua_ptys.rs`: opaque owner-scoped handles, contained cwd, bounded writes/output/dimensions, native `PtySession`, reader thread emits immutable events only.
- DAP broker in `desktop/src/lua_dap.rs`: registration-authoritative command, env-clear process, process-group teardown, bounded Content-Length frames, stderr/output events, owner teardown.
- Git adapter in `desktop/src/lua_git.rs`: fixed argv without shell for status/diff/blame/branches/history/worktrees/checkout/branch, safe relative paths/refs. Hunk mutation and worktree creation intentionally rejected pending native retained-diff / destination-picker authority.
- Native Agent read bridge returns status, sessions, and bounded active-session messages. Mutations fail explicitly because they require native approval/workflow UI. Existing agent protocol/trust/Lua runner untouched.
- Credential handles remain unavailable unless a native broker exists; no environment-variable secret storage.

## Trust model
- Manifest supports sandboxed Lua, trusted subprocess, and trusted native declarations plus exact package/revision/workspace approvals.
- Trusted subprocess entrypoint activation remains blocked until a host-owned persisted approval store is wired.
- Trusted native ABI and dynamic Tree-sitter parser loading remain blocked: no existing audited loader/ABI/lifetime manager exists; do not add ad hoc dlopen/LoadLibrary.
- Remote PTY/Git/syntax remain blocked pending daemon-owned opaque protocol identities.

## Verification
`cargo check -p neoism` passes after Tree-sitter, DAP, Git, and Agent bridge integration. Existing warnings only.

## Remaining polish
- Completion resolver is explicit; keyboard acceptance does not yet defer to resolve-before-insert.
- Task/test output is streamed but not parsed into retained problem/test/coverage models.
- DAP state is transported but not retained as native threads/frames/scopes/variables models.
- Git query results stream bounded output rather than structured parsed payloads.
