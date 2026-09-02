---
name: "Repeated subagent completion notifications — FIXED"
description: "Malformed subtask completion metadata is repaired without daemon panic or duplicate delivery"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-24"
updated: "2026-08-24"
---

Release hardening added 2026-08-24: malformed persisted `extra.subtaskCompletions` can no longer panic completion publication. `normalize_subtask_completions` repairs null/scalar metadata to `[]`, unwraps recoverable arrays/objects and JSON-encoded strings, accepts `records`/`completions` wrappers, synthesizes stable MessageIds for record-shaped entries lacking valid IDs, and deduplicates by notification ID and execution generation. Valid arrays remain unchanged.

Repair runs both at generation publication and during parent/startup reconciliation. Repairs are persisted under the existing per-child keyed lock and emit SESSION_UPDATED. Existing same-generation records clear a matching owed marker and reconcile pending delivery without emitting another completion event. Parent reconciliation repairs metadata before legacy-overlap retirement and queue scan; startup now visits every plural-key session, including malformed non-arrays. Valid new-array history remains authoritative over the legacy singular record, which is retired rather than double-delivered.

Focused regression covers null, object, JSON-string metadata through startup repair plus repeated publication, verifies recoverable IDs survive, valid arrays are installed, owed marker clears, and only one parent queue row exists. Existing valid legacy overlap test remains passing. Full session_actions suite: 16 passed. cargo check, rustfmt --check, and git diff --check pass. Only session_actions.rs changed for this hardening; workflow/frontend were not touched.
