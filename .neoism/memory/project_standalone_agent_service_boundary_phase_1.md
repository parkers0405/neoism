---
name: "Standalone Agent service boundary Phase 1"
description: "Phase 1 executable service boundary implemented; Agent server has zero direct neoism-extensions production references."
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

Implemented Phase 1 on branch `neoism_agent_v2` without commit. Added `neoism-agent-service-api` with `ExecutableService`, request/result/source/purpose/error types, `AgentServices`, and std/PATH/PATHEXT resolver. Added `neoism-agent-neoism-adapter`; it snapshots `neoism-extensions` managed binaries and isolates the current reconciliation side effect behind a TODO. `AppState` owns injected services; production `listen` now requires services. CLI uses standard services; desktop child/parent and workspace daemon use the Neoism adapter. LSP, formatter, bwrap lookup, platform shell, and external-agent runtime lookup route through the service. Removed server dependency/imports for neoism-extensions and deleted old managed_lsp_path/Rio lookup. Checks and targeted tests pass; existing unrelated warnings remain.
