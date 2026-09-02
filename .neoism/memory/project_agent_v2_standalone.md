---
name: "Agent V2 standalone service boundary complete"
description: "V2-only/Turso-only cleanup + injected service API and Neoism adapter shipped on neoism_agent_v2; scoped daemon proxy auth and daemon-only supervision"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-25"
updated: "2026-08-25"
---

## Status

Completed and pushed on branch `neoism_agent_v2`.

- Initial platform commit: `c428c7b34`.
- Standalone boundary cleanup: `f92b082ff` (`refactor(agent): complete standalone v2 service boundary`).
- Production HTTP API is `/v2` only; legacy route modules and handwritten legacy OpenAPI removed.
- Persistence is Turso/libSQL only; SQLite/sqlx backend selection, FTS fallback, and `NEOISM_AGENT_DB_BACKEND` removed.
- Added `neoism-agent-service-api` with injected config, executable, workspace search, notes, docs, memory, semantic-memory, context, and built-in MCP contracts.
- Added `neoism-agent-neoism-adapter` owning grouped GUI config projection, vault notes/memory semantics, product docs, and managed extension executable resolution.
- Agent server has zero direct `neoism-workspace-index`/`neoism-extensions` dependencies and zero process-global config/search registries.
- Neoism product docs moved to `neoism-product-docs`.
- ACP, CLI, desktop, daemon, SDK callers use canonical V2 paths; canonical error aliases and non-persisted config aliases removed.
- Daemon `/agent` proxy now authenticates local/paired callers and mints short-lived HMAC scoped Agent credentials; forged/expired/wrong-audience and cross-directory access tests pass.
- Workspace daemon is the sole port 4096 supervisor; desktop is client-only.
- Goals/subagents route gating and disabled workflow workspace watching fixed.
- Prompt `author` restored in V2; no-reply idempotence and active-run steering both verified.

## Verification

- `cargo check -p neoism-agent-server -p neoism-agent -p neoism -p neoism-workspace-daemon` passed.
- TypeScript SDK workspace build passed.
- OpenAPI/router parity, contract fingerprint, `git diff --check`, targeted auth/proxy/service/plugin/prompt tests passed.
- Full serial Agent suite reached all relevant tests green after targeted fixes; two pre-existing platform-path tests remain Linux/Windows expectation issues noted before this cleanup.

Three unrelated frontend edits remained uncommitted and were excluded from the commit:
- `neoism-frontend/shared/src/panels/agent_pane/view/markdown/inline_style.rs`
- `neoism-frontend/shared/src/panels/agent_pane/view/markdown/tests.rs`
- `neoism-frontend/shared/src/panels/agent_pane/view/side_panel/sections.rs`.
