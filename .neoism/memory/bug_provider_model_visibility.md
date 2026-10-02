---
name: "Provider model visibility by auth path — FIXED"
description: "Codex OAuth models now account-authoritative; disconnected Zen no longer advertises or uses anonymous free models"
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-01"
updated: "2026-08-01"
---

2026-08-01: Fixed provider model visibility so listings follow the actual auth path.

OpenAI ChatGPT/Codex OAuth no longer receives the full models.dev OpenAI API catalog with subscription pricing pasted over it. `ProviderRegistry::openai_model_access` now fetches the authenticated `https://chatgpt.com/backend-api/codex/models` catalog (Bearer + ChatGPT-Account-Id), refreshes OAuth through the same shared helper used by streaming, retains only `visibility: "list"` slugs, intersects those IDs with models.dev metadata, and then applies Codex limits/zero cost. API-key auth still gets the full platform API catalog and API limits/prices. The account catalog has a 5-minute identity-scoped success cache, 5-second request timeout, 2 MiB response cap, and 15-second fail-closed error cache. Override endpoint: `NEOISM_AGENT_OPENAI_CODEX_MODELS_URL`.

Immediate regression and fix: initially sent Neoism's package version (`0.7.111`) as the Codex `client_version`. The backend interprets that as an ancient Codex CLI and silently returns HTTP 200 `{"models":[]}`. Live probes confirmed no query gives 400, `0.100.0` gives one model, and `0.200.0`/`999.0.0` give the full account catalog (8 visible subscription models for the tested account, with pro models absent). Discovery now defaults to `client_version=999.0.0` because Neoism performs its own models.dev compatibility intersection; override with `NEOISM_AGENT_OPENAI_CODEX_CLIENT_VERSION` if needed. Never use `CARGO_PKG_VERSION` for this endpoint unless Neoism adopts Codex's version scheme.

OpenCode Zen remains in `/connect`, but while disconnected its `/v2/providers` row has no models/default and `/v2/providers/configured` omits it. Removed both prior anonymous paths: `usable_provider_catalog`'s zero-cost model exception and `ProviderRegistry::provider_auth`'s synthetic API key `public`. Connected Zen still lists free and paid active models.

Direct `neoism-agent models` and the interactive CLI provider-model list now consume `/v2/providers/configured`, matching desktop/shared pickers.

Primary code: `neoism-agent-builtins/src/provider_openai.rs`, `provider.rs`, `provider_catalog.rs`, `provider_service.rs`; CLI in `cli_direct_commands.rs` and `chat_session.rs`. Verification after the regression fix: `cargo check -p neoism-agent-builtins` passes; all 7 Codex-focused tests pass; corrected file rustfmt and diff checks pass. Earlier full verification had all 90 built-ins tests and `cargo check -p neoism-agent` passing.
