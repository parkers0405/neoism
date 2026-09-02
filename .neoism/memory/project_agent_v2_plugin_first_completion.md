---
name: "Agent V2 plugin-first completion"
description: "Agent V2 plugin-first platform completed and pushed; zero central route switches, providers/agents/config physically extracted, generation-owned lifecycle, full verification passed."
type: "project"
scope: "project"
origin: "session implementation and verification"
created: "2026-08-25"
updated: "2026-08-25"
---

# Agent V2 plugin-first cutover completed

Branch `neoism_agent_v2` pushed through `96f03127a`.

Final architecture:
- Production API is canonical `/v2` only; central plugin route switches are zero.
- First-party commands, skills, agents, providers, config, prompts, goals, VCS, artifacts, interactions, semantic, workflows, subagents, LSP, MCP, PTY, workspace tools, notes, and custom tools register through public plugin contracts.
- PTY WebSocket uses a transport-neutral public WebSocket contribution bridged generically by Axum.
- Concrete provider implementations/catalog/auth/OAuth/routes/transforms/tests live in `neoism-agent-builtins`, not server.
- Agent catalog/native prompts/config interpretation live in builtins, with server consumers resolving pinned snapshot services.
- Full config merge/Markdown discovery/normalization lives in bootstrap config plugin; duplicate server parser removed.
- `INTERNAL_PLUGINS`, central plugin router, direct kernel tool fallback, user-visible kernel tools, duplicate config/provider/agent implementations, and old route modules are deleted.
- Workspace state is opaque plugin-ID keyed; generation teardown hooks remain behind Arc generation leases and run exactly once after retirement, with explicit shutdown disarming drop hooks.
- Neoism GUI and workspace daemon use the same canonical shared Agent path.

Verification:
- Strict `-D warnings` checks passed for plugin API, builtins, and server.
- Plugin architecture/conformance tests passed.
- Builtins provider suite: 69 passed.
- Full server suite: 405 passed (environment-dependent LSP tests ignored).
- Contract generator tests and TypeScript SDK build passed.
- Standard `cargo check -p neoism-workspace-daemon -p neoism` passed; desktop retains unrelated pre-existing warnings under `-D warnings`.

Push: `74c9152ad..96f03127a` to `origin/neoism_agent_v2`.
