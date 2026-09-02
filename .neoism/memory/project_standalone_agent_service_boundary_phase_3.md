---
name: "Standalone Agent service boundary Phase 3"
description: "Phase 3 moved Neoism memory behind optional adapter service; Agent server now owns only semantic index interface and has zero workspace-index dependency."
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

Phase 3 completed uncommitted on `neoism_agent_v2`. `neoism-agent-service-api` now has optional MemoryService DTOs, SemanticMemoryIndex, async built-in MCP dispatch, and SystemContextFragment. Neoism canonical vault memory behavior/MCP/context moved into `neoism-agent-neoism-adapter`; only `<scope>/Memory` and `<default>/Memory/Personal` are used at runtime, with no legacy `Personal/Memory` fallback or migration. Agent server implements semantic memory indexing over existing Turso `memory_embeddings` rows and injects it through the narrow interface. Memory discovery/config/calls/context are generic registry-driven and honest when absent/disabled. Workspace-relative memory file-read redirect was removed. Server has no neoism-workspace-index source, Cargo, or dependency-tree reference. Canonical file/frontmatter/index and semantic path/root/hash/model row compatibility retained. Checks passed for service API, adapter, server, CLI, desktop, daemon; adapter memory linked/default/user tests; server MCP/context/semantic/V2/OpenAPI/contract tests; git diff --check.
