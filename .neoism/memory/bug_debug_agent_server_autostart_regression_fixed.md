---
name: "Debug Agent server autostart regression fixed"
description: "Daemon-owned Agent supervisor was only WebSocket-lazy, leaving native debug Agent unreachable; fixed with eager daemon startup and proxy fallback."
type: "bug"
scope: "project"
origin: "post-Agent-V2 debug launch regression"
created: "2026-08-25"
updated: "2026-08-25"
---

# Debug Agent server did not auto-start

Fixed on `neoism_agent_v2` in commit `86ad95a5a`.

Root cause: commit `f92b082ff` correctly removed desktop-owned Agent child-process spawning when ownership moved to the workspace daemon, but the daemon's existing retrying supervisor was only invoked lazily by WebSocket `AgentClientMessage` dispatch. Native desktop Agent traffic uses direct HTTP/SSE and bypasses that dispatch, so `agent_server::ensure_started_for_request()` only probed and waited for a process nobody started.

Fix:
- Embedded desktop daemon eagerly calls `neoism_workspace_daemon::agent::ensure_agent_server_started()` after listener readiness.
- Standalone workspace daemon eagerly starts the same supervisor during `run()`.
- Authenticated daemon reverse proxy calls it defensively before forwarding.

Do not restore desktop `spawn_agent()` or put supervisor startup inside `server::router()`: daemon owns Agent lifetime, and router construction occurs in tests.

Verification: `cargo check -p neoism -p neoism-workspace-daemon`, daemon `agent_proxy` tests (2 passed), and diff hygiene passed.

Operational note: launching `target/debug/neoism` while an installed Neoism instance is already the active single instance may forward to that installed process and exit. Also, old installed Agent servers expose `/global/health`; V2 probes `/v2/health`. Debug profiles normally use isolated dynamic ports/sockets.
