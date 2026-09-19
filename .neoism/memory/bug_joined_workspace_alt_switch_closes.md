---
name: "Joined workspace Alt-switch closes PTY — FIXED"
description: "Fixed peer PTY session IDs being reattached/routed to HOME on Alt+number workspace switches; endpoint ownership defenses and regression tests."
type: "bug"
scope: "project"
origin: "implementation 2026-09-17"
created: "2026-09-17"
updated: "2026-09-17"
---

# Alt+number joined workspace switch closed remote PTY — FIXED

## Symptom
After joining a workspace, Alt+1 back to a local workspace produced `Remote PTY: unknown session ...`, `Remote terminal detached ... Nothing was replayed`, and the joined terminal/tab disappeared.

## Exact cause
Workspace selection correctly retained grids and parked daemon connections, but `rehydrate_remote_routes_for_attached_daemon` treated any HOME attachment (`!link_is_peer`) as owning every grid. On peer→HOME switch it sent the joined workspace's peer-local PTY session IDs to HOME. HOME returned `unknown session`; the generic PTY failure path invalidated the peer binding and emitted child-exit, which drove normal tab teardown. A second hazard allowed remaining PTY frames from a drained source batch to be applied after an earlier workspace frame switched the active endpoint.

## Fix
- Rehydrate only grids for which `grid_uses_attached_daemon` is true: HOME owns non-adopted grids; adopted grids are owned only by their durable endpoint.
- Added route-level endpoint ownership checks for rehydrate fallback, PTY failure, transport gating, creation, close, and output.
- App PTY/PtyFailure dispatch now compares captured source endpoint with the currently attached endpoint, dropping stale frames after an in-batch switch.
- Connections remain parked/restored; no command replay was added. Explicit leave/close behavior is unchanged.

## Files
- `neoism-frontend/desktop/src/app/mod.rs`
- `neoism-frontend/desktop/src/context/manager/daemon_sessions.rs`
- `neoism-frontend/desktop/src/context/manager/ingest.rs`
- `neoism-frontend/desktop/src/context/manager/test.rs`

## Verification
- `cargo check -p neoism` passes (pre-existing warnings only).
- Regression tests pass: `home_daemon_does_not_own_joined_workspace_routes`, `wrong_endpoint_unknown_session_cannot_close_joined_terminal`.
- `git diff --check` passes.
