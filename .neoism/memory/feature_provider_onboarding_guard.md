---
name: "Connect-first provider onboarding"
description: "Strict new-chat onboarding and real no-provider hints; unknown existing Rust sessions defer to server, scoped/aged readiness lifecycle avoids false catalog blockers"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

# Connect-first provider onboarding

Provider onboarding exists in the TypeScript GUI and Rust desktop/shared composer. Confirmed-empty authoritative `/v2/providers.connected` replaces home hints with `/connect to connect a provider`. Never infer readiness from health (`app.connected`) or stored account count: environment and no-auth providers count as connected. Unknown catalogs are not known empty.

Important native follow-up: see bug_agent_readiness_old_chat.md. EXISTING Rust sessions must not be blocked because the client's catalog is absent, expired, or loading; the server resolves persisted session provider/model. New drafts still require authoritative usable readiness before creating a conversation, preserving draft/attachments on rejection. Known fresh no-provider and invalid-model results still block generation entry points. Unknown new-draft copy says availability is loading, not /connect. Previous tests that required unknown old sessions to reject were deliberately updated.

Native readiness now has independent lifecycle from draft defaults/model catalog. It is scope-proven by endpoint/credential/directory/revision, retains original age, coalesces workers, and reconciles existing-chat/session-directory changes. No clearing on every defaults/model/connect picker refresh. Negative or selected-model-unusable authority expires after30s; usable positive lasts5min. Readiness refresh failures cannot replay an expired negative catalog. Prepared/queued prompts validate their captured model using the model-specific freshness accessor. External CLI sources remain exempt.

TypeScript: useAppController scopes provider catalog by server/token/directory. prepare(target,true) waits catalog/default/session hydration and validates before sessions.create, uploads or prompt; rejects with /connect for actual disconnection or /models for unusable model; retries failed hydration. Local /connect remains usable. selection.ts does not fall back to disconnected providers for explicit empty connected arrays. App's standalone ComposerFooter and Composer both need the onboarding hint.

Rust api_mapping::ProviderReadiness captures connected IDs and model options. Shared view/user_input trait exposes no_providers_connected for the reused desktop painter. Guards run before clearing input/attachments, echo/streaming or session outbound. Native commands guard prepared/queued sends, new-session creation, forwarded commands, goals and FX. Errors are transient notices, not transcript system messages. Native fixtures must install readiness with CatalogScope and checked time instead of direct Option assignment.

Historical initial verification: GUI source tests736 and TypeScript check; shared/native cargo check; original shared suite2855 passed, ten unrelated failures, one ignored. Latest follow-up: four shared readiness regressions and five isolated cache regressions pass. Full native follow-up verification currently blocked by unrelated app/mod.rs SSH/system-switch edits, as recorded in bug_agent_readiness_old_chat.md. No release build.
