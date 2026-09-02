---
name: "Standalone Agent service boundary Phase 2"
description: "Phase 2 notes/docs services and generic built-in MCP registry completed; Agent server workspace-index references are memory-only."
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

Implemented Phase 2 on branch `neoism_agent_v2` without commit/push, preserving existing V2/Turso and executable-boundary work. `neoism-agent-service-api` now owns dependency-light optional `NotesService` and `DocumentationService` contracts, backend-advertised stable scope IDs/labels, operation DTOs, and a generic `BuiltinMcpService` registry in `AgentServices`; standard services inject none. `neoism-agent-neoism-adapter` now owns Neoism vault/default/linked/all notes behavior and notes/docs MCP bridges. Product docs bundle ownership moved to new `neoism-product-docs`; workspace-index consumes it only to seed editable Welcome docs. Agent server deleted `mcp_notes.rs`/`mcp_docs.rs`, removed notes/docs config name branches, and dispatches injected built-ins generically for status/connect/tools/resources/prompts/calls and config projection. First-class notes delegates to `NotesService`, vanishes absent/disabled, and graph/index compatibility no-ops were removed. Generic prompts no longer assume notes/docs. Adapter and fake-server tests cover behavior, absent/disabled discovery/calls, MCP registry, docs, and notes. Checks passed for service-api, product-docs, adapter, server, CLI, desktop, daemon; MCP/notes/docs/V2/OpenAPI parity and diff-check pass. Agent server's only remaining `neoism_workspace_index` source references are four in `mcp_memory.rs` (lines 454, 547, 548, 567).
