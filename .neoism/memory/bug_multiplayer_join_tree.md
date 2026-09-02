---
name: "Multiplayer join wrong-root + delayed tree — FIXED"
description: "Root causes and fix invariants for wrong-root flashes and delayed remote file trees during multiplayer workspace joins"
type: "bug"
scope: "project"
origin: "investigation and implementation in Neoism workspace"
created: "2026-07-10"
updated: "2026-07-10"
---

# Multiplayer join + remote file tree root causes

## Fixed 2026-07-10

The reported same-room 3-10 minute delay was not network latency. It was lost/ignored invalidation plus unsafe cross-daemon join state.

### Desktop
- Peer selection now resolves peer ownership before local grid lookup, preventing workspace-id collisions from activating a local grid.
- Attaching a peer daemon is read/adopt-only: it no longer ensures sessions or publishes local workspace grids/root paths to the peer.
- Peer attach clears daemon-specific host/workspace/tab/session snapshots from the previous connection.
- Pending peer adoption completes only from the newly attached daemon's full `HostWorkspaceTree`, and only with an absolute `root_dir`.
- Adoption requires an authoritative absolute daemon root; removed the fallback to active local pane cwd/root.
- Missing peer `daemon_url` now fails visibly instead of falling through to local adoption.
- Failed redial disarms `pending_peer_adopt`.
- `sync_daemon_workspaces` and `ensure_daemon_sessions_for_all_routes` are peer-link no-ops, preventing later lifecycle calls from leaking local state.

### Web
- Both WASM Command+P workspace selection and DOM workspace switcher resolve the owning host daemon URL and switch daemon before workspace adoption.
- Pending remote adoption completes only when the new daemon publishes the workspace with an absolute root; expected root is checked across the switch.
- PTY cwd/root hints no longer define Explorer root. `WorkspaceSummary.root_dir` is the authority.
- Unsolicited `FilesServerMessage::Changed` is treated as a root-scoped invalidation and immediately calls WASM `refreshFileTree`; it is no longer forwarded as an unmatched request_id 0 service reply.
- Pending service mappers clear when TerminalPanel is disposed.

### Verification
- `cargo check -p neoism-ui -p neoism`
- `cargo test -p neoism-workspace-daemon --test files_smoke` (3 passed)
- web `npm run typecheck`
- web `npm test` (59 passed, including 3 new root-scoped invalidation tests)
- web `npm run build` / wasm-pack succeeded before the final test helper extraction; subsequent typecheck/tests passed.

### Known repository-level issue
A broad `cargo test -p neoism-workspace-daemon --tests` still hits unrelated pre-existing integration fixture compile failures (`workspace_receive` missing `shared`, `presence_ws` missing `auth`). Do not confuse those with this files-plane change.
