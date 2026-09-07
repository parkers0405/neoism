---
name: "Windows service readiness and recovery"
description: "Windows service readiness protocol + single background recovery owner; actual-endpoint request helper; bounded discovery; checks pass, Windows test-target cfg blockers remain"
type: "bug"
scope: "project"
origin: "session implementation"
created: "2026-09-07"
updated: "2026-09-07"
---

Implemented Windows service startup/readiness/recovery in desktop service_process.rs, main.rs, embedded_daemon.rs, agent_server.rs, tailscale.rs; daemon linked-agent supervisor hardened in neoism-workspace-daemon/src/agent.rs. No commit/release/push.

Service owner is a process-local Weak/Arc registry + background monitor. Child stdin is an early parent-death leash; stdout carries `NEOISM_WORKSPACE_SERVICE/1 READY` or ERROR, logs go to config/log/workspace-service.log (initialized before internal-service dispatch). Readiness requires child acknowledgement AND HTTP /health exact daemon identity, not TCP connect. GUI Windows waits at most 3s but retains pending endpoint/owner and background retries (1..30s backoff). Startup deadline 120s accommodates hosted association up to ~96s; responding service isn't timed out. Owned unhealthy-after-ready service restarts after 30s. External occupied endpoints are not marked ready, not killed, and not competed with by repeated child spawns. Unix existing socket attachment / first-frame fast path retained.

Request helper signature: `ensure_started_for_request(server: &str) -> Result<(), String>`, worker-thread only. Uses actual endpoint including base path; bounded reqwest health probes, body cap, URL-safe errors; acquires the same local owner only after failed owned-local root endpoint probe. Remote/proxied /agent URLs never auto-start a local service. API owner integrates authenticated-host readiness with registered credential separately. Live health JSON uses provider_credential_store (snake_case), despite OpenAPI camelCase; accept both.

Tailscale uses direct hidden CLI with 2s per-candidate deadline, file-backed bounded stdout capture, kill/reap on timeout (no pipe reader stuck on inherited descendants); cached/blocking probe now single-flight. tempfile already dev dependency is also native normal dependency. Mac app-bundle fallback keeps a separate 2s candidate budget. main updater tasklist now uses existing hidden helper; palette sibling_binary checks .exe on Windows.

Daemon agent supervisor remains process-local one-shot owner, monitors an inner listen task so startup panic/exit retries; initial health deadline 120s, never timeout healthy running listen. Health identity validation, redirects disabled; /agent path/credential/query/remote URLs excluded from local bind ownership; IPv6 loopback fixed.

Validation: native and Windows MSVC production cargo check PASS. 12 desktop readiness/discovery/recovery tests + 2 daemon ownership/health tests PASS native. Windows `--tests` check blocked by existing windows keybindings cfg(not(test)) export and unconditional DaemonEndpoint::Unix in context/manager/test.rs and daemon_client/mod.rs. Native Windows GUI/service acceptance not performed; no release build.
