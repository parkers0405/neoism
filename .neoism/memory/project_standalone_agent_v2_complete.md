---
name: "Standalone Agent V2 complete"
description: "Agent V2 is complete: V2-only API, Turso persistence, structural plugins, injected product services, generated SDKs, verified and pushed as 2b10cd15e."
type: "project"
scope: "project"
origin: "session final verification and push"
created: "2026-08-25"
updated: "2026-08-25"
---

# Standalone Agent V2 architecture

Completed and pushed on branch `neoism_agent_v2` in commit `2b10cd15e` (`Complete standalone Agent V2 architecture`).

Durable invariants:
- Production Agent HTTP surface is canonical `/v2` only; OpenAPI is authoritative and covers all 130 router operations.
- Agent persistence is Turso/libSQL only. Compatibility is retained only for shipped persisted message data, including V0 subtask completion migration and prompt queue delivery migration.
- Canonical Agent config is camelCase only. Product GUI config projection belongs to `neoism-agent-neoism-adapter`.
- Optional plugin availability is structural and derived from immutable per-workspace generation snapshots. Disabled contributions have no routes, tools, context, runtime state, workers, or execution fallback.
- Runtime state for MCP, LSP, PTY, workflows, semantic indexing, background jobs, and subagents is per-AppState/per-workspace and deterministically torn down.
- Child sessions created outside normal subagent creation must be registered in the workspace subagent lifecycle before completion publication.
- Every host-visible executable is resolved through injected `ExecutableService` before launch.
- Product language catalogs live in `neoism-agent-neoism-adapter`; standalone Agent defaults to an empty injected catalog.
- FFF workspace search lives in `neoism-agent-workspace-search-fff`; server has no direct FFF dependency.
- Generated TypeScript clients are frontend-neutral and transport-independent.

Final verification:
- Full `neoism-agent-server` serial suite: 467 passed, 5 environment-dependent LSP tests ignored.
- Strict `RUSTFLAGS=-D warnings` Agent checks passed.
- Desktop and workspace daemon checks passed.
- Workspace daemon proxy auth tests: 2 passed.
- OpenAPI check, generator tests, and all TypeScript SDK package builds passed.
- `git diff --check` passed.

The three unrelated frontend edits in markdown inline style/tests and side-panel sections were intentionally left uncommitted.
