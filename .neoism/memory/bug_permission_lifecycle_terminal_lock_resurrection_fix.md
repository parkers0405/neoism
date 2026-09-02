---
name: "Permission lifecycle terminal-lock resurrection fix"
description: "Late permission events cannot resurrect terminal-locked frontend child branches; explicit transition semantics preserve real task_id continuation."
type: "bug"
scope: "project"
origin: "frontend lifecycle implementation"
created: "2026-08-24"
updated: "2026-08-24"
---

Frontend branch lifecycle now distinguishes `AuthoritativeRun` from `AncillaryActivity` in the shared side-panel transition API. Permission requests/replies use ancillary semantics, so a completed/terminal-locked child stays Completed when late permission cleanup arrives. Desktop `permission.replied` and shared wasm `ToolUseResult` route through `note_permission_replied`; permission requests use the same guarded transition. Genuine child runtime Busy/Active (`note_subagent_runtime` / subagent status-start edges) remains authoritative and clears the lock for task_id continuation. Focused shared and desktop tests cover late reply, stale request, and genuine continuation. Affected frontend cargo checks and focused tests pass; existing unrelated warnings remain. Full `cargo fmt --all -- --check` became blocked by concurrently unformatted `neoism-agent-server/src/workflow.rs`; all affected files pass direct rustfmt check and `git diff --check` passes.
