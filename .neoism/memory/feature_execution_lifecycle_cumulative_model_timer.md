---
name: "Execution lifecycle + cumulative model timer"
description: "Final blockers: durable-armed cleanup guards, active-execution subtask CAS, deletion-safe family SSE"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-26"
updated: "2026-08-26"
---

Final production-blocker pass completed. Cleanup guards now retain payload/state through the durable await and disarm only after a successful or definitive idempotent result. `SubtaskAdmissionGuard` retains the requested terminal status for Drop retry; `ProviderSegmentGuard` clones segment identity and retries on cancellation; `ExternalRunGuard` uses a new durable-first `try_finish_session_run` so in-memory ownership is not removed before Turso completion. Deterministic tests abort each explicit finish while the Turso writer is held and verify Drop retry completes.

`register_execution_subtask` now returns `ExecutionSubtaskRegistration::{Inserted,AlreadyPresent,Rejected}` from a transaction whose INSERT SELECT requires the exact root/execution row with finished=0. Family revision advances only for insertion. Guard admission rejects stale/terminal registration and prevents launch. Cross-AppState test finishes execution A, rejects stale branch insertion with no row, then successfully admits execution B.

Root-family SSE now keeps connection-owned known membership, merges current descendants, and removes a member only after delivering its authoritative durable `session.deleted`. This handles DB deletion preceding event persistence while never adding unrelated sessions. Regression opens a root stream, deletes unrelated+child, verifies only child deletion arrives, and verifies reconnect runtime hydration has no deleted branch.

Strict CARGO_BUILD_JOBS=2 server/protocol/daemon/shared/desktop/WASM checks pass. Focused server guard/CAS/SSE tests and protocol/daemon tests pass. git diff --check passes. No commit/push; unrelated edits preserved.
