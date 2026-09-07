---
name: "Desktop /connect freeze/unavailable — asynchronous scoped auth workers"
description: "Desktop /connect full auth flow off UI thread; scoped cancellation + notifier, actual endpoint readiness and authenticated health, Windows COM browser worker"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-07"
updated: "2026-09-07"
---

Desktop `/connect` originally synchronously traversed execute_slash_text -> open_connect_picker -> catalog/auth HTTP -> ensure_started_for_request (up to 8s startup wait) + 5s blocking reads. Fixed in desktop/src/neoism/agent/pane/connect.rs: typed ConnectRequest/ConnectOutcome, PendingConnect with Arc<AtomicBool> identity, existing AgentBackgroundSender notifier and drain_background_updates. Catalog, account lookup/model reconciliation, account mutations, credential PUT/POST, OAuth authorize/manual callback/auto callback, and browser launch all run on short-lived workers. Static loading/error+Retry picker; existing picker shimmer stops after 1.5s; Enter cannot consume loading picker. ESC/new request/other picker/new session/session switch/server reset/tab close cancel ownership; late completion cannot restore UI or account selection. Request checks cancellation again AFTER readiness, before credential mutation; already-issued HTTP cannot be rolled back. Auto OAuth now retains a cancellable waiting picker instead of unscoped background completion. Windows browser worker initializes/balances a COM STA (Win32_System_Com dependency feature); Unix uses unchanged browser helper.

API coordination: startup owner provides agent_server::ensure_started_for_request(server: &str) -> Result<(), String>, actual URL incl proxy prefix. api.rs propagates errors; registered authenticated joined endpoints use same bearer-aware transport for health as actual requests (otherwise anonymous health wrongly rejects healthy authenticated proxy). No timeout extensions. agent_server.rs/startup files belong to separate startup work and were not edited for this fix.

Regression coverage: pane/connect_tests.rs has 14 tests (blocked catalog returns promptly + notifier, unavailable/retry, dead health, key re-entry, account actions, auto/manual OAuth, stale/cancel/scope drift, authenticated proxy, healthy remote with dead configured local in isolated subprocess, idle loading animation, async model account selection, cancellation during readiness prevents PUT). Full desktop agent suite: 227 passing via cargo test -p neoism --bin neoism neoism::agent:: (NOT --lib: desktop lib does not include GUI agent). Existing three event HTTP mocks in updates.rs now serve health before gated history responses; model transition unit test invokes post-reconciliation apply_model_with_connection. cargo check -p neoism --tests passes. No commits/release builds.
