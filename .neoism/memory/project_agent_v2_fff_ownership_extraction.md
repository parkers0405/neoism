---
name: "Agent V2 FFF ownership extraction"
description: "FFF workspace search ownership moved out of neoism-agent-server into an instance-owned Agent adapter crate; server now has zero FFF refs/dependencies."
type: "project"
scope: "project"
origin: "neoism_agent_v2 ownership extraction session"
created: "2026-08-25"
updated: "2026-08-25"
---

Uncommitted on neoism_agent_v2. Added `neoism-agent-workspace-search-fff`, owning the picker registry, FFF find/grep/directory logic, streaming fallback, mmap/cache env behavior, engine identity, cancellation/bounds/ignore handling, and pin lifecycle. Server now has transport-neutral workspace-search tool wiring only and zero FFF source/dependency-tree references. CLI standalone server/ACP/TUI and Neoism services explicitly compose the adapter; desktop finder/file mentions use the adapter directly. Adapter tests pass 5/5 and git diff --check passes. Full server/CLI/daemon/desktop and server isolation/no-warm test runs are currently blocked by unrelated concurrent breakage in `custom_tool.rs` (literal `@@`, missing function) and `session_routes.rs` (undefined `state`). Three unrelated shared frontend files were preserved. No commit/push.
