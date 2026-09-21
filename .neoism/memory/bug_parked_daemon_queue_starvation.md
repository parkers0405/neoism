---
name: "Parked daemon queue starves PTY/LSP — FIXED"
description: "Root cause and fix for host/guest terminal+LSP starvation: parked daemon queues retained PTY output forever; filter parked inbound and remove global pre-auth backlog."
type: "bug"
scope: "project"
origin: "live broken host/guest diagnosis and regression tests"
created: "2026-09-21"
updated: "2026-09-21"
---

# Joined workspace PTY/LSP starvation from parked queues

## Root cause
Each desktop window keeps inactive daemon sockets in `WindowServerSession::parked_connections` so background editors survive server switches. `DesktopDaemonConnection::drain_editor_messages` drained its entire inbound queue, extracted editor replies, then reinserted every non-editor message. A parked socket retained its old websocket-local PTY subscriptions, so terminal output accumulated forever. Every UI pump copied the entire growing queue and each new PTY frame generated another render wake. In the live broken host/guest session, each machine had one active bidirectional socket plus one parked socket that had received the same ~4.73 MB stream while sending nothing for about six minutes. The resulting time-dependent UI pump starvation stopped terminal and LSP dispatch while the workspace daemon and separate Agent transport remained healthy.

## Fix
- `DesktopDaemonConnection` has an atomic parked/editor-only mode. Its inbound task drops non-editor families before queueing or waking the UI while parked.
- `drain_editor_messages` now consumes and drops any stale non-editor frames already queued rather than reinserting them.
- `Application::complete_server_switch` marks outgoing connections parked and restored connections active.
- Reactivation remains safe because daemon-link attachment explicitly sends `AttachPty`, which subscribes the socket and returns retained backlog, and requests current workspace state.

## Related protocol defect fixed
`handle_socket` previously emitted `SessionRegistry::backlog_messages()` globally on every fresh websocket before authentication or socket-local attachment. Removed that eager replay and the now-unused method. PTY backlog is now only returned by explicit `AttachPty`. Added integration coverage proving a fresh unauthenticated socket receives no unrelated PTY frames.

## Verification
- `cargo test -p neoism --bin neoism parked_drain_drops_stale_pty_frames_instead_of_growing_forever`: 1 passed.
- `cargo test -p neoism-workspace-daemon --test workspace_ws_integration fresh_socket_never_receives_unattached_pty_backlog`: 1 passed in 0.33s.
- `cargo check -p neoism-workspace-daemon`: passed.
- `git diff --check`: passed.

The first version of the integration test hung only because its test-created zsh PTY was not explicitly closed; cleanup was added with `ClosePty` and websocket closes.
