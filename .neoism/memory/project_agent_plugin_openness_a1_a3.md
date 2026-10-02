---
name: "Agent plugin openness A1-A3 process-v2 wave"
description: "Process v2 now adapts six unary native services through PluginHost, supports exact-owner reverse host RPC, bounded cancellation/stream foundations, and TS author APIs; provider/routes and Agent Lua remain open."
type: "project"
scope: "project"
origin: "implementation"
created: "2026-10-06"
updated: "2026-10-06"
---

---
name: "Agent plugin openness A1-A3 process-v2 wave"
description: "Process v2 now adapts six unary native services through PluginHost, supports exact-owner reverse host RPC, bounded cancellation/stream foundations, and TS author APIs; provider/routes and Agent Lua remain open."
type: "project"
scope: "project"
origin: "implementation"
created: "2026-10-06"
updated: "2026-10-06"
---

Implemented an additive `neoism-plugin/2` wave while preserving the A0 process owner/trust work. `process_v2.rs` now declares AgentService list, CommandService list, SkillService list, SystemContextService sections, PromptService render, and ConfigService load, plus exact-owner plugin-to-host frames, host replies, cancellation and generic stream envelopes. Old tool/hook/event frames and empty initialize responses remain wire compatible.

`plugin_host_process.rs` turns subprocess declarations into ordinary `PluginContributions`; `PluginHost` performs metadata stamping, conflict checks, generation lifecycle, and retirement. Reverse RPC distinguishes method-bearing requests from replies, validates exact `{pluginId, instanceId}`, rejects retired/wrong owners, bounds frames/pending maps/reverse queues, and supports timeout/tool cancellation via `$/cancel`. Scoped methods use `PluginContext`: config get/set, workspace read/write/list, and event publish. No broad bearer token is inherited by subprocesses.

Production grants now install read-only config snapshots, symlink-safe rooted relative workspace access, and namespaced event publication. Workspace/package host paths are not returned by the new service API. TypeScript `@neoism/plugin` has service declaration author APIs, scoped `context.host`, exact instance identity, bounded concurrency/frames, host request timeout/cancellation, and stream envelope types. Existing optional SDK client remains for source compatibility, without inheriting the server bearer token.

Verification passed: process_v2 wire tests (5), process host tests (8), rooted host-service security tests (2), `cargo check -p neoism-agent-plugin-api -p neoism-agent-server`, SDK `npm run typecheck`, and `scripts/plugin-host.test.mjs`. Existing unrelated server warnings remain.

Still open: process providers/routes/MCP/message parts and real streaming parity; dynamic revocation and richer opaque resource handles; owner-scoped storage/network/process brokerage; dedicated isolated Agent Lua subprocess (A4). A4 was not started because safe manifest entrypoint/discovery and Lua budgets should build on the still-incomplete provider/route contract rather than linking `mlua` into the server or creating a misleading partial runner.
