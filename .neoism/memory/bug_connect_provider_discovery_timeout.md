---
name: "Provider /connect timeout fixes, pending readiness contract"
description: "Source fixes verified: legacy5s /connect overrides, duplicate readiness GET, joined/OAuth budgets, nonblocking catalog/auth refresh with explicit unknown readiness; affected Windows machine still unvalidated"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Provider connection timeout follow-up

User reports `Connect unavailable` / `Neoism request timed out waiting for the server response` / `Connection failed` on a separate computer, suspected Windows. This Linux workspace is NOT a reproduction. No affected-machine logs, Windows runtime testing, release build or deployment were performed.

Exact strings still came from native /connect after healthy agent: connect.rs retained five-second provider/auth/account/authorize overrides after the earlier admission fix; joined proxy can wait120s for host startup inside that clock. Fix: connect_api_request30s owned-local work /150s selected or credential-bearing; auto OAuth360s local /480s selected. api::uses_local_startup_readiness centralizes configured endpoint ownership + registered credential checks. Errors carry static stage labels. Warning fields stage/class/elapsed/wire budget only; no URLs, bodies, account IDs, raw response errors or warnings on normal Esc cancellation.

Duplicate request bug: open_connect_picker called refresh_provider_readiness before its own catalog worker, launching two provider GETs and surviving Esc. Picker catalog already applies scoped readiness; redundant launch removed. Blocked-catalog wake/cancel regressions failed before removal and passed afterward. Do not restore duplicate launch.

Daemon server.rs: only POST v2/providers/:id/oauth/callback gets345s headers, ordinary120s and SSE no total lifetime.120 startup+345 headers<480 selected client. agent.rs IPv6 bind target now bare ::1 not URL brackets; test parses IpAddr.

Builtins provider_catalog.rs: cold/corrupt/offline raw metadata falls back to new provider_catalog_fallback.json (basic popular providers, NOT full current model guarantee); models.dev refresh and configured model discovery run background single-flight. Failed endpoint discovery caches cooldown and keeps old known IDs. Provider service connects from raw basic metadata, retaining account-effective OpenAI models.

Builtins provider_openai.rs: codex_models_with_credentials uses try_lock cache ownership and background model discovery. Background expired OAuth uses original static REFRESH_LOCK with bounded acquisition +15s token HTTP/body deadline, scoped credential re-read, token persistence before releasing guard. Cache key includes bearer hash/account/endpoint/version; refresh publishes rotated identity and rechecks cache after scoped refresh to avoid stale caller overwriting good metadata. Failed discoveries have failure TTL. Important integration correction: immediate empty account-model lists must NOT become authoritative negative readiness and deadlock generation-owned refresh. Provider List now emits readinessPending when connected OAuth OpenAI metadata is empty; shared api_mapping::ProviderReadiness::from_json returns None (unknown) for it. No invented usable models. Regression proves expired OAuth refreshes and model metadata converges without generation.

OpenAI auth preparation/token exchange15s, browser complete330s, device polling300s overall. Callback retry remains fresh authorization, no blind POST replay.

Verified Linux: desktop connect28 passed, readiness filter30 passed, request_tests filter14 passed, configured credential-bearing local subprocess fixture1 passed; daemon proxy2 + IPv6/local ownership1 passed; builtins OpenAI15 passed, hung catalog1 + provider-list/auth-method hung catalog1 passed; shared pending readiness1 passed. Final production cargo check neoism + workspace-daemon + builtins passed after last cache fix. See docs/agent-startup-responsiveness.md follow-up. No full-suite/Windows/release claim.
