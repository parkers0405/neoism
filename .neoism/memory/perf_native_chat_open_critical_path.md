---
name: "Native chat opening critical path"
description: "Removed global metadata scan, duplicate100+80 history, serial image gate and UI JSON parse;32 subscribed baseline;97 native+30 server+9 shared and cargo check pass; GUI latency not benchmarked"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Native conversation-open critical path

User asked why a Rust GUI/backend cannot open saved chats instantly. Full native click -> pane -> metadata/history HTTP -> store -> ingestion -> timeline audit found actual overfetch/completion barriers, not evidence of a global read mutex. Ordinary local native history uses agent HTTP, not daemon WS. SQL pagination and viewport-only exact timeline measurement already existed; do not replace virtualization generically.

## Implemented

Single GET /v2/sessions/{id} no longer calls catalog-wide project/list_sessions. catalog_activity::project_one uses SessionStore::catalog_family_members, canonical parent/execution-root expression indexes and bounded indexed frontier queries (Turso does not support recursive CTE). Preserves exact existing family membership, off-tree root hints, children, cycles/orphans, runtime-only branches, authorization and hosting. Activity uses active_run(root), not cloning all active runs. New state/catalog_family_tests.rs checks equivalence and proves unrelated unhydratable session cannot break single-session GET.

Initial native history limit is API_INITIAL_HISTORY_LIMIT=32 stored messages (was competing100 preload +80 SSE snapshot). Newly created root subscriptions own the one initial snapshot AFTER subscribing; navigation skips competing preload history but still starts independent metadata. Reused streams/child navigation still fetch their own catch-up. New subscriptions also own initial runtime reconciliation, rather than activation plus first-connect both doing it. Keep subscribe-before-snapshot correctness barrier; never blindly disable reconnect recovery.

Preload metadata/history arrive separately: SessionMetadataLoaded, history-only SessionPreloaded with raw_count/requested_limit, SessionPreloadFinished. Worker slots remain occupied until both requests complete, but transcript can publish first. Metadata failure does not discard authorized text. Metadata applier lives in pane/hydration.rs and preserves source/model/directory/readiness/imported history/plan behavior. Identical direct preloads do not invalidate warm layout.

SSE snapshots carry a separate HistoryPageLoaded pagination update so stored count, not expanded render-block count, determines has_older; newest refreshes preserve older paging cursors. HistoryRefreshFailed explicitly evicts untouched display previews even though root's legacy preload is no longer doing the authoritative read. Snapshot application strips untouched preview blocks, then merges remaining live parts even on a first cold page with no cursor. A new regression caught the initial implementation dropping a post-SQL live part; fixed and rerun.

fetch_session_messages_page now returns text/artifact descriptors immediately. api/generated_images.rs: two-worker bounded128-job nonblocking image queue; scoped endpoint/credential/auth-generation cache, single flight,64entries64MiB cache and96MiB cached+queued payload reservations; 25MiB image limit on top of32MiB transport streaming bound,5min success/5s failure TTL. Prefer recent images, use captured credential, reject arbitrary external artifact paths, recheck generation on consumption. Image-only compare-and-replace matches existing message ID+original URL+MIME and cannot replay stale text or resurrect removed messages. SSE callbacks use their ordered stream queue; preloads/older/undo pages schedule image-only background patches. Saturation retains descriptors for event-driven retry using completion/progress notifications, stops admission at first full queue, and never blocks UI. Download failures do not spin perpetual retries. Cache/pending reservations bound worker side; UI-owned transcript attachments are separate.

Warm preview cache stores predecoded Arc<SessionMessagePage>, not JSON parsed on click. Preparation, resident-byte accounting and block clone happen on fetch worker before cache mutex; peek clones only Arc. Cache still30s/16entries/8MiB total/1MiB entry, scopes endpoint+credential+session+auth generation, display-only, no stale cache as server authority. Charges decoded allocation capacity, nested composition children, payload references and icon allowance conservatively. Native fetch no longer serializes entire JSON for caching or reparses/remaps it for preview; only perf logging serializes to count wire bytes. API cache remember accepts only freshly mapped page BEFORE image hydration/live merging.

NEOISM_AGENT_UI_PERF_LOG or NEOISM_AGENT_PERF now logs `agent conversation first transcript layout` with selection-to-first-layout elapsed_us, block count and preview flag. This is NOT GPU-present timing. Existing fetch/layout timings remain.

## Verification

Final targeted native command passed97 tests: `NEOISM_SERVER=invalid://agent-unit-tests cargo test -p neoism --bin neoism -- conversation_open generated_images conversation_cache provider_readiness readiness_tests defaults_tests conversation_latency_ responsiveness_tests config_chip_ catalog_cache updates::tests pane::history_tests imported_history_preload preloaded_disjoint returning_to_warm_cache older_timeline --test-threads=1`. Includes real mock HTTP/SSE opening held metadata test proving text appears and only one limit32 baseline is requested, slow-artifact independence/dedup, auth scoping/oversize rejection, preview deletion/failure/live preservation, cursor retention, no-op epoch, readiness/defaults/provider guards, undo and stream races.

30 server catalog tests passed: `cargo test -p neoism-agent-server catalog_ -- --test-threads=1`. One older catalog stream test assumed first body frame was activity, but unchanged protocol sends family then activity; corrected test to wait for activity within a timeout, retaining running/idle/root/foreign assertions. No runtime protocol weakened.

Nine shared readiness/startup config-chip tests passed. Final `cargo check -p neoism --bin neoism` passed with existing warnings; git diff --check passed. Transient concurrent GPU, status_sync and notes_sidebar compile errors cleared without edits to those files. Full native-agent filter attempt hit240s timeout without a retained result, so only claim targeted97, not entire native suite. No release build, app launch, GUI click-to-present benchmark or fabricated latency numbers. This also supersedes the old native-verification blocker in bug_agent_readiness_old_chat.md: those native readiness regressions now genuinely passed.

## Remaining measured-risk boundaries

Cold server/network startup and real remote RTT cannot be zero.32 stored messages is not a byte/part budget: one huge tool output or embedded image can still dominate payload, mapping and ingestion. UI ingestion's deadline is between updates, not within a single large update. Large family child preloads still exist; cross-family parked layouts intentionally invalidate; cold layout still estimates all loaded sources. Images are detached and newest-first, not yet viewport-requested thumbnails; loaded UI attachments and visible original-resolution decoding can still cost memory/CPU. New-pane constructor still has synchronous prompt-history/config/theme reads. No proof of global read mutex; store reads use separate Turso connections and can experience DB busy retries. Need real first-layout/GPU-present traces before claiming one-frame instant or choosing next optimization; preserve existing viewport virtualization, auth scope and live-event races.
