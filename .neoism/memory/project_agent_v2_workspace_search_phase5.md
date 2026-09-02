---
name: "Agent V2 Phase 5: injected workspace search"
description: "Phase 5 workspace search service injection completed; FFF is instance-owned default and Agent server has zero global search/index registries"
type: "project"
scope: "project"
origin: "2026-08-01 implementation session"
created: "2026-08-24"
updated: "2026-08-24"
---

Phase 5 completed on `neoism_agent_v2` without commit. `neoism-agent-service-api` now defines required, transport-neutral `WorkspaceSearchService` plus warm/pin lifecycle, file-find, grep, directory search, cancellation, pagination/bounded metadata DTOs. `AgentServices` requires an injected workspace search. Agent server `FffWorkspaceSearchService` owns an instance-local bounded picker registry; no search/index `OnceLock` or singleton remains in Agent server. Standard server and Neoism adapter services inject FFF. grep/glob, streaming fallback, provider warming, and directory options route through `state.services.workspace_search`; workspace plugin disablement prevents warming/tools and directory-options returns 404. Prompts/tool descriptions are engine-neutral. Tests include fake service two-AppState isolation, directory-option injection, no-warm disablement, and FFF pin lifetime. Desktop mention picker owns its service/pin per pane (no replacement global). Scoped grep uses an explicit workspace root so result paths remain workspace-relative. Preserve the known unrelated prompt 204 assertion for final cleanup.
