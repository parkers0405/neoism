---
name: project-v2-hardening-wave
description: "2026-08-26 wave fixing the four real V2 risks — run-ownership unification, auth session-id guessing, plugin OpenAPI parity, desktop TLS"
metadata: 
  node_type: memory
  type: project
  originSessionId: 39912326-ef2c-4184-a6b4-eec06c374bd3
  modified: 2026-08-26T21:58:06.108Z
---

Agent V2 hardening wave (2026-08-26, committed 193b395c0 + fc907abd5 + 3ea496df5), all four "real risks" from the architecture audit FIXED:

1. **Run ownership unified.** `InnerState.runs` map DELETED; `SessionCoordinator` is the single in-memory authority (new `active_run`/`active_runs`/`install_run` alongside try_start_run/finish_run/abort_run). ~25 sites ported (session_run/context/actions/queue/prompt/undo/routes/execution_activity/tool_runtime/external_agent + tests); the `wait_until_session_not_running` adoption shim deleted. Durable `session_runs` table remains as persistence, not authority. Recovery/restore paths use `install_run`.

2. **Auth session-id guessing killed.** `session_id_from_path` returns None for `/v2/plugins/...`; plugin session scope resolves ONLY via `find_scoped_plugin_session` (shared by auth's `resolve_scoped_plugin_session` AND `plugin_route_dispatch` directory resolution), which requires a `RouteScope::Session` descriptor binding the exact segment as `session_id`. A plugin resource id colliding with a session id can no longer authorize or teleport dispatch into that session's workspace. Tests: `workspace_scoped_plugin_route_ignores_colliding_session_id_segment`, `session_id_from_path_never_guesses_on_plugin_routes`.

3. **Plugin OpenAPI parity closed.** New `every_plugin_route_descriptor_is_in_openapi_and_vice_versa` (openapi.rs) builds the real production plugin snapshot and bidirectionally compares its route descriptors (shape-normalized via `normalize_path`) against the spec's plugin-owned paths — the previously-exempt ~72 operations are now drift-gated. Gotcha: drop the generation lease before `state.shutdown()` or lease drain times out.

4. **Desktop TLS shipped.** New `neoism-frontend/desktop/src/neoism/agent/transport.rs`: `AgentTransport::{Plain(TcpStream), Tls(rustls::StreamOwned)}` keeping sync blocking-socket semantics (WouldBlock polling; handshake driven explicitly with 5s deadline before installing the caller's read timeout). `parse_http_server` (api.rs + agent_server.rs) accepts `https://` (default 443); both HTTP + SSE constructors and the health probe use the transport. rustls 0.23 ring-provider + webpki-roots (matches lockfile; NO aws-lc-rs). SSE/chunked decoding unchanged.

Follow-up (3ea496df5): all 6 pane tests fixed — optimistic queue bump in submit/start_goal_prompt, older-page latch released on stale session, warm-switch adopts cached layout epoch (invalidate only cross-family), stale expectations updated (PersistConfigChoice on model change, round-tripped client message id, .git/index filtered).Residual: embedded_daemon unix test needs port 4096 free (stale release process squats it).

Related: [[bug-agent-double-stream-tail]], [[bug-execution-timer-inherited-new-run]]
