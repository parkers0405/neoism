---
name: "Old chats falsely blocked by provider catalog readiness"
description: "Fixed old-chat catalog-not-ready regression: cancelled bootstrap + unscoped/expired readiness; unknown existing sessions use server authority, caches reused; native verification blocked by unrelated SSH edits"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

# Old chats falsely blocked by provider catalog readiness

User reported intermittent native toast: `Provider catalog is not ready. Use /connect to connect a provider or retry after it loads.` when sending in old conversations and after switching chats. This was a client-side unknown-readiness regression introduced during native loading optimizations, not an authoritative server report of disconnection.

Root chain: sidebar creates a pane for an unopened existing chat, then `discard_queued_draft_defaults` removed `ApplyConfigDefaults`. The only provider-readiness worker was scheduled inside execution of that cancelled command. Separately, cached readiness was seeded only if unrelated config defaults were fresh (30s). Saved-directory hydration changed scope without reconciling readiness, while already-applied Option had no provenance. All send entry points rejected unknown readiness, so pressing Enter initiated recovery by loading both models and readiness instead of sending.

Repair: native readiness lifecycle independent of draft defaults/models; directly seed scoped readiness from its own cache; retain same-scope known state; track CatalogScope, original checked_at, and per-scope in-flight workers; coalesce requests and reconcile on existing-chat selection, directory activation/hydration, and scope changes. /connect catalog results carry scope; opening/closing Connect and defaults do not blank readiness. Model picker cannot renew age from an unaged cached tuple. External CLI conversations skip builtin readiness refresh.

Admission policy (native and shared): unknown local catalog never blocks an EXISTING session; its server resolves persisted provider/model. New drafts still require an authoritative usable provider/model before creating a conversation. Unknown new-draft notice says availability is loading and preserves the draft, not /connect. Known fresh no-provider and invalid-model results still guard all paths. Native prepared/queued dispatch uses model-specific provenance and freshness, including when captured model differs from current composer.

Freshness: negative/unusable-selected-model authority lasts30s, usable positive lasts5min. `cached_provider_readiness_with_age` preserves original insertion age across panes; stale negative/unusable snapshots are not admission authority. Readiness refresh uses strict cache get_fresh, so failures cannot replay expired disconnects. Other picker presentation retains last-good fallback. Context-limit lookup reuses known configured-model metadata up to last-good expiry instead of rediscovering providers on each chat navigation. Scope remains endpoint+API credential+directory+invalidation revision; auth/config mutations invalidate appropriately.

Native APIs in pane/input.rs: current_provider_readiness, current_provider_readiness_for_model, apply_provider_readiness, refresh_provider_readiness, refresh_provider_readiness_for_model. API helpers cached_provider_readiness(_with_age), fetch_provider_readiness_scoped; catalog_cache has snapshot_age and get_fresh. All production native assignments funnel through apply_provider_readiness. Native test fixtures must install with current scope rather than assign Option directly.

Validation in this increment: four shared provider-readiness tests passed (including unknown-existing first send); five isolated tests of actual catalog_cache.rs passed (strict expired refresh errors/backoff, age, scoping, single flight, invalidation). Added eight native lifecycle tests, native first-send/stale queued-model regressions, API independent-readiness/negative-expiry/context-reuse tests, and updated scoped fixtures. Native cargo test/check attempts reached desktop but were blocked by unrelated in-progress SSH/system-switch code in app/mod.rs around8186 (`attach` missing / malformed iterator chain); earlier transient unrelated shared PR/palette compile errors cleared. Fixed our own missing percent_encode import in connect.rs; no readiness-file errors remained in latest native check diagnostics. No unrelated files were modified to unblock compilation. No release build or live GUI run.

When rerunning native tests, set `NEOISM_SERVER=invalid://agent-unit-tests` to avoid constructors starting readiness requests against the user's actual local server. Suggested filter: `cargo test -p neoism --bin neoism -- provider_readiness readiness_tests catalog_cache defaults_tests conversation_latency_ conversation_cache responsiveness_tests --test-threads=1`. Keep the one-clock Alt+A startup animation from feedback_agent_startup_animation_once.md.
