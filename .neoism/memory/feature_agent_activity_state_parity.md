---
name: "Desktop/web Agent activity-state parity"
description: "Root busy/hydration carries QueueUpdate metadata; shared atomic part ingestion drives Crafting/Pondering/Tinkering only from semantic evidence."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-03"
updated: "2026-09-03"
---

# Desktop/web Agent activity-state parity

Implemented daemon-to-shared web activity semantics to match desktop.

- Root `session.status busy` is forwarded as `QueueUpdate` with queue count/preview and `startedAt`, not a manufactured `StreamingState::Working`.
- Running-state hydration uses the same QueueUpdate shape, preserving the server clock and never claiming tool work without part evidence.
- Shared queue status uses neutral `Generating`/`Crafting` when it must hydrate an otherwise-idle live run, while preserving an already-active semantic state. `QueueStatusDecision.started_at` now anchors the live shared pane clock as it already did for cached/desktop state.
- Shared live ingestion has atomic `ingest_live_part_message` and `ingest_live_part_delta` APIs. Assistant text => Generating/Crafting, reasoning => Thinking/Pondering, tool/subtask => Working/Tinkering.
- WASM catalog routes MessageStart/ContentDelta/MessageUpdated through atomic shared ingestion. Daemon delta forwarding preserves text/reasoning/tool classification.
- Desktop queue consumers use the same shared Generating fallback policy.

Tests added for ordered busy -> assistant -> reasoning -> tool -> assistant transitions in daemon, shared pane, and wasm catalog, plus running-state hydration metadata.

Verification: focused daemon/shared tests pass; affected package checks and desktop lib check pass; wasm32 test binary compiles. Actual wasm execution was unavailable because `wasm-bindgen-test-runner` is not installed. Scoped rustfmt and git diff checks pass. Full workspace fmt remains blocked by unrelated pre-existing formatting changes elsewhere.
