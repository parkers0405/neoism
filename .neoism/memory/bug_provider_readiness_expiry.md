---
name: "Loaded provider falsely still loading"
description: "FIXED age-only desktop admission expiry caused loaded provider to become unknown at1/30/300s; freshness now only schedules revalidation;42 readiness+20 cache tests pass"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-24"
updated: "2026-10-24"
---

# Loaded provider incorrectly reported still loading

User confirmed slow-Windows model catalog fix works, then reported random cross-platform desktop notice: Provider availability is still loading. Your draft is unchanged; retry shortly, after sitting with everything loaded.

Root cause proved in desktop pane/input.rs current_provider_readiness_for_model: age-only expiry returned None for pending >=1s, negative >=30s, positive >=300s despite stored scope-valid proof. submit.rs started asynchronous refresh then immediately rejected unknown new-chat state. Existing chats bypassed unknown rejection. Failed refresh retained data but not usability; apply_provider_readiness pending retention used age-filtered getter and process cache complete-to-pending protection ended at300s. Not a network latency defect.

Fixed: current_provider_readiness_for_model now checks scope, not time. Separate provider_readiness_is_fresh_for_model keeps original1/30/300 thresholds for background revalidation. Complete authoritative rejection also remains valid while revalidating. Rejected cached pending replies do not overwrite original validation timestamp. Cache retained_snapshot_age reads scope-valid retained readiness beyond generic presentation STALE; complete-over-pending retention requires same nonempty readinessIdentity and connected set, no age cutoff. True endpoint/directory/credential/revision changes invalidate; complete disconnect/model removal replaces prior positive. No larger timeouts or unknown autoqueue. Queue gate validates captured queued model.

Files: desktop agent/pane/input.rs, api.rs, api/catalog_cache.rs, api/catalog_cache_tests.rs, pane/readiness_tests.rs, pane/tests.rs, docs/agent-startup-responsiveness.md. No renderer changes.

Verified final readiness filter42pass independently parent rerun; catalog_cache20pass (overlaps); cargo check -p neoism --bin neoism passes; git diff --check passes. Tests cover301s first-submit queues exactlyonce no toast and starts refresh; failed refresh keeps readiness age; expired complete pending downgrade rejected; pending2s accepted model stays usable; authoritative disconnect/removal blocks; directory/endpoint/credential invalidation; captured queued model. No release build/deployment/Windows runtime test.
