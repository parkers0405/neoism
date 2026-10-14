---
name: "Whole-family catalog activity colors"
description: "LEFT catalog aggregates whole family: white model/subagents, red background-only jobs, yellow permissions; typed fresh authority/live SSE, terminal-child busy residue guarded, native/web checks passed."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Whole-family colored activity in left Conversations catalog

Supersedes parent-only running bool in [[bug_left_catalog_main_running_state]]. User confirmed root spinner worked but switched off while a subagent still ran. CatalogActivity in agent-core/session.rs is Idle, Running, Background, Permission (snake_case serde). Priority is pending permission anywhere in family -> yellow, model/provider/coordinator/queue/outstanding subagent -> white, background-only live jobs -> theme red, otherwise idle/no spinner. Branch runtime_status stays separate.

Server catalog_activity.rs projects existing atomic family/runtime snapshots, durable outstanding branch lifecycles, provider segments, coordinator ownership/queue, pending-permission map and authoritative background registries. Never infer activity from old tools or !execution.finished. Transport-only CatalogSessionInfo flattens stored SessionInfo plus catalogActivity; root list/info JSON includes this field without persisting it. Scoped SSE subscribes before baseline reads, then freshly reprojects runtime/background/status/permission and metadata events as session.catalog.activity {sessionID,activity}. Cross-directory children map to authorized owning root; root directory/claims/host hydration and lag/expiry controls preserved. Existing lease repair loop publishes affected-root runtime snapshots, no new supervisor.

Stale terminal child Busy ledger could mask idle/background even after lifecycle ended. Added guard for completed/failed child fallback statuses; positive coordinator/provider/worker/queue/resume authority still wins. Real external/manual child resumes claim coordinator ownership before Busy. Permission replies support requestID and requestId; real HTTP durable-only/sessionless fallback is tested.

Shared entry has optional catalog_activity, typed live overrides mutate both stored and displayed roots without moving viewport/order/selection; survive stale pages/metadata and clear on reconnect/deletion/invalidation. Native parses canonical new activity events; raw session.status cannot erase family activity. Root square geometry, alpha trails, clipping and visible-only redraw ownership unchanged; details/task launch remain white; followups remain muted continuation arrows.

Web ThreadSummary carries optional catalog_activity string while busy remains shipped fallback only if activity absent. Daemon reuses authenticated SSE proxy/handle map with directory catalog keys, emits global CatalogActivity messages and metadata ThreadUpdated/ThreadDeleted, does not proxy parent idle as family idle. Global list requests subscribe discovered loaded directories. Wasm applies activity before current-chat scoping and reads typed field from thread list; no extra agent-core dependency, serde type inferred via shared setter. Connection failures log for catalog instead of inserting agent error bubbles. No token-driven catalog polling.

Verification: protocol integration 2 passed; shared left_catalog 4, color 1, visible-square gating 1; native left_catalog 1, catalog_activity 2; server catalog_activity 8 and scoped catalog metadata 1. Native/shared/daemon/server checks and wasm32/web check plus whitespace check pass. Daemon catalog_activity unit tests blocked by existing E0425 private todo_items_from_response helper in agent/tests.rs (0 executed); production daemon check passes. No release build, live visual validation, dedicated expired-subtask repair notification or new lag/expiry test. Concurrent work preserved.
