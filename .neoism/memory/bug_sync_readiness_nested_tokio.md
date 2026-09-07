---
name: "Sync readiness must support entered Tokio contexts"
description: "Synchronous agent readiness isolates nested Tokio calls on a std worker and uses shutdown_background to avoid DNS/runtime-drop hangs"
type: "bug"
scope: "project"
origin: "parent review follow-up"
created: "2026-09-07"
updated: "2026-09-07"
---

Follow-up to bug_windows_service_readiness_recovery.md: desktop agent_server::ensure_started_for_request is synchronous and must not assume the caller is outside Tokio. Runtime::block_on inside an async task panics; ordinary runtime Drop can also panic there, or wait indefinitely for blocking DNS after HTTP timeout.

Fix confined to agent_server.rs: public helper detects Handle::try_current; with an entered context it runs the private readiness implementation on a named plain std worker and joins it. Plain workers keep the direct fast path. The private runtime uses shutdown_background() after block_on so OS DNS resolution cannot extend the readiness deadline through Runtime::drop. No block_in_place (would panic on current-thread runtimes). Existing 500ms probe/8s local startup budgets unchanged; this remains a synchronous worker-only API, NOT an async UI API.

Added tests invoking the sync helper from current-thread Tokio, a multithread Tokio task, and spawn_blocking, with success and bounded-unresponsive endpoints. Mock server runs independently on a std thread because any synchronous caller necessarily pauses its current-thread executor. Added deterministic configured-vs-actual port/prefix ownership test. Existing process-local service owner registry/static remains single-owner; remote/wrong-port/proxy calls cannot acquire it. api.rs credential-aware wrapper and main/UI code were not changed by this follow-up.

Verified new Tokio tests plus existing agent_server tests native; native and Windows production cargo check pass. Audit: API uncredentialed branch calls helper, credentialed branch has its own std transport; desktop connect/refresh workers currently use std::thread, but generic sync helper should also be safe for entered runtime callers.
