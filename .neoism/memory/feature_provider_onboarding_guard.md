---
name: "Connect-first provider onboarding"
description: "No-provider home hints become /connect; guards retain draft and prevent conversation creation across TS/native/shared, authoritative catalog scoping"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

Provider onboarding now exists in both TypeScript GUI and Rust desktop/shared agent composer. Confirmed-empty authoritative `/v2/providers.connected` replaces home `tab agents / commands` hints with `/connect to connect a provider`. Never infer from health (`app.connected`) or stored account count: environment and no-auth providers count as connected. Unknown catalogs must not be treated as known empty.

TypeScript: `useAppController.ts` scopes rendered provider catalog by server/token/directory. `prepare(target, true)` waits catalog/default/session hydration, validates provider/model readiness before `sessions.create`, uploads or prompt, rejects with /connect (no providers/disconnected selected provider) or /models (no model), retries failed catalog hydration. Local /connect remains usable. `selection.ts` no longer falls back to disconnected providers for explicitly empty connected arrays. App renders a standalone ComposerFooter because its Composer has showFooter=false; wire needsProvider to BOTH.

Rust: shared `api_mapping::ProviderReadiness` captures connected IDs and model options; persisted on desktop/shared pane, reset during catalog refresh/server changes. Desktop config/model/connect hydration populates it; shared ApplyConfigDefaults queues RefreshConnectProviders, WASM ConnectProviderCatalog applies it. Shared `view/user_input.rs` trait/macro exposes no_providers_connected for reused desktop painter. Submit checks before clearing input/attachments, echo, streaming or session outbound. Desktop commands.rs guards prepared/queued prompts (queued model explicitly validated), session creation, forwarded server commands, goals and FX. External CLI conversation sources exempt. Readiness failures use transient notices, NOT transcript system messages, so home remains home.

Verified: all GUI source tests (736) + TypeScript check; cargo check -p neoism-ui -p neoism --tests; new shared readiness regressions and submit tests pass. Full shared UI suite had 2855 passed, 10 unrelated failures, 1 ignored (tree width, markdown layout/spellcheck, child/mobile navigation, chip animation reason, palette float geometry, host completion). Desktop runtime tests compile-checked, not executed. No release build.
