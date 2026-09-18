---
name: "desktop workspace reconnect"
description: "Golden-standard desktop same-runner workspace reconnect: transport loss vs terminal death"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-15"
updated: "2026-09-15"
---

Desktop `/session` reconnect now treats transient websocket loss as transport, not terminal death.

Behavior:
- DaemonClient stays on one runner; Open is only after accepted HelloAck.
- Application Ping/Pong + read/liveness timeout recycle zombie sockets; backoff is bounded full-jitter.
- PtyFailureClass::Transport gates input (await_attach, no replay) and keeps session identity; Terminal/unknown-session still invalidates the view.
- Same-runner HelloAck resyncs existing PTYs (generation-guarded AttachPty), CRDT snapshots, git, file tree. Auth reject and HostEnded stay fatal at the app layer; SSH tunnel rebuild is unchanged.

Primary files: desktop daemon_client/mod.rs, context/manager/{ingest,daemon_sessions}, remote_pty.rs, app/mod.rs, screen/{code,markdown}_crdt.rs.
