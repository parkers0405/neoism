---
name: "Neoism TypeScript SDK golden standard"
description: "One-install SDK with capability-gated optional plugins and GUI-parity session-family streaming"
type: "project"
scope: "project"
origin: "Agent session 2026-08-31"
created: "2026-08-31"
updated: "2026-08-31"
---

# Canonical plugin-aware TypeScript SDK

The public consumer story is one install: `@neoism/sdk`; `@neoism/plugin` remains separate for plugin authors. Optional Agent Server capabilities remain plugin-owned and are represented by capability-gated `PluginSdk` clients rather than guaranteed core methods.

Golden-standard SDK increment implemented in the worktree on 2026-08-31:
- `client.plugins.use()` / `tryUse()` with workspace-aware capability discovery and `CapabilityUnavailableError`.
- `@neoism/sdk-plugin-builtins` typed clients for optional agents, commands, providers, skills, goals, LSP, MCP, PTY, semantic search, VCS, workflows; subagents stay in compatibility package with list/stop only.
- `@neoism/sdk` re-exports all optional clients; users do not import internal packages.
- Session-scoped SSE already follows the whole descendant session family, so external clients observe main-agent and subagent activity exactly through one stream. Main agent owns spawning.
- PTY now has typed WebSocket transport with one-use connect tickets, output/cursor decoding, resize, and browser-safe server auth bypass limited to exact ticketed PTY connect upgrades.
- Core facade response `unknown`s tightened; config defaults and current background-task contract drift synchronized.
- SDK examples, consumer tests, clean tarball install, package metadata, and npm provenance publishing added.

Verification passed: OpenAPI check; npm typecheck including examples; SDK tests; cargo check for server; focused PTY ticket auth test; clean packed install/import; npm publish dry-run; diff check. Whole-workspace `cargo fmt --check` still fails on broad pre-existing Rust formatting unrelated to this increment.

These changes are not yet committed or published. The prior canonical `@neoism/sdk` shell commit is `36617ddce`.
