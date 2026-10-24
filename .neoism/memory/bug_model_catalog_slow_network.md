---
name: "Single GPT-5 picker on slow network"
description: "Fixed tiny offline catalog, retryable background hydration, authoritative Codex IDs and same-picker updates; tests/native/wasm pass; full Windows check blocked by concurrent diff_card API edits"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-24"
updated: "2026-10-24"
---

# Single GPT-5 model catalog on poor internet

User confirmed previous /connect fix opens immediately on separate Windows computer; /model and /models showed only GPT-5 with poor internet. Not locally reproduced on Windows hardware.

Root cause: tiny bundled catalog (four providers, only OpenAI gpt-5), one-shot models.dev fetch with no warm-read retry, successful network data dependent on disk persistence, provisional desktop configured-model cache treated as final (30s fresh/300s stale), and account wire IDs filtered against raw metadata instead of materializing missing account models.

Implemented full versioned public models.dev snapshot with provenance, SHA-256 09659a0eeb6685e6554b93a4897b7dede514b52fd6f5d9731dc2e241359074d7, upstream MIT license in misc/licenses/models.dev.txt and NOTICE. Warm reads schedule single-flight background retries with bounded exponential backoff; valid network data publishes in memory independently of best-effort disk persistence; response capped at 32MiB. Common effective resolver materializes account-authoritative Codex IDs with conservative capabilities/limits, zero subscription cost, configured overrides, and request-scoped API/account isolation. Scoped connected_ids uses request AuthStore. Successful empty catalogs remain distinct from pending/failed discovery.

Both provider List and Configured return root catalogStatus {source:bundled|disk|override|network,state:loading|ready|retryable-error,revision,retryAfterMs,message?}, separate from readinessPending. Desktop provisional cache bypasses normal completed freshness/stale-success fallback. Shared/desktop/wasm propagate status and correlated requests; same open picker revalidates without losing query or selection. Directory/scope/picker ownership guards reject stale replies. Hydration permits 60 seconds elapsed rather than 12 replies (old cap lasted ~6s), honors server backoff, expires individual requests after20s, and revokes both desktop Arc identity and shared request ID. Offline state retains options, stops animation, ASCII title Models offline; /model to retry. /connect remains nonblocking.

Verification: 147 builtins library tests passed; final added snapshot checksum test passed; 4 desktop model_hydration tests passed; frontend agent's 7 final shared lifecycle tests passed; protocol legacy response compatibility test passed. Final native production cargo check and wasm32 check passed before unrelated renderer edits. Builtins Windows xwin check passed. Full Windows frontend/daemon xwin check and additional parent shared-test rerun FAILED due concurrent unrelated PreparedSyntaxLine API mismatches in shared widgets/diff_card.rs (Option<Rc> accessed directly) and widgets/diff_card/measurement_cache.rs tests (obsolete spans construction). Left concurrent files untouched. No full Windows build clearance or affected-machine runtime validation. No release build, commit/deploy. Docs: docs/agent-startup-responsiveness.md.
