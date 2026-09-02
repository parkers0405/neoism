---
name: "project_standalone_agent_service_boundary_phase_4"
description: "Phase 4 injected configuration boundary and Neoism adapter projection"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

# Agent service Phase 4 config boundary

Implemented on `neoism_agent_v2` without commit. `neoism-agent-service-api` now owns `ConfigSourceService`, workspace snapshots, ordered projected layers, stable IDs/identity, discovery roots, writable targets, and source-ID updates. Standalone canonical config is JSON-only: user `$XDG_CONFIG_HOME/agent/agent.json` and workspace `.agent/agent.json`; no aliases or server-global env content.

`neoism-agent-server` config loading/roots/writes are service-driven; old `config_sources.rs` discovery was deleted. Config service is threaded through AppState/runtime consumers (config, instruction, skill, agent/command, workflows, tools/plugins, MCP, LSP adapter snapshots). MCP/config writes use snapshot source IDs. Context epochs include config snapshot identity. Tests include fake-service AppState isolation and generic GUI/terminal boundary.

Neoism-specific projection lives in `neoism-agent-neoism-adapter/src/config.rs`: grouped user `config.json` projects only `agent`, with `terminal.shell` fallback; canonical extension `mcp.json` and existing project `neoism.json` remain adapter-owned. Adapter writes preserve unrelated GUI groups. These product filenames are not generic Agent aliases.

Audit: generic server has zero `NEOISM_AGENT_CONFIG*`, config.jsonc/neoism.json(c), terminal-block reads, or ConfigSource globals. `git diff --check` clean. Requested cargo checks pass. Focused config/instruction/skill/workflow/MCP/LSP/V2/OpenAPI/adapter tests pass. One broad pre-existing native plugin test reaches the dirty V2 prompt contract and expects 200 while route returns 204; not a config failure.
