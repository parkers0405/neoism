# Agent database activity audit

## Confirmed source-level amplification

- `catalog_activity::project` used to call `list_sessions()` for every response, including an empty or single-row root page. That query selected every canonical `info_json`, including potentially large saved context/tool snapshots, and hydrated all hosting associations. Non-root listing already loaded all sessions before calling this projection, so it performed the enumeration twice.
- `CatalogProjection::project_event` repeated the same global canonical read for every session create/update/delete event, per catalog SSE subscriber. Metadata updates therefore amplified reads independently of content searches.
- Catalog SSE startup/reconnect previously loaded all canonical session JSON. It now uses existing compact list summaries once their persistent backfill is complete. Execution-root overrides are recovered with an expression-index range query; legacy databases retain the canonical fallback until backfill completes.
- Untitled external-session title repair loaded and deserialized the entire transcript to find its first user message. It now selects only the first user message in session-position order, with `LIMIT 1`. This can still examine preceding non-user rows; it does not return/decode trailing history.
- Search maintenance could run `OPTIMIZE INDEX conversation_search_fts` every 15 minutes even with no new projection work. It now performs the existing one-time idle maintenance for legacy segments, then requires productive projection work before another successful maintenance run. Idle settling, writer/search admission, retry backoff and cancellation boundaries are unchanged.

## Current bounds and retained behavior

Catalog pages use the existing indexed `catalog_family_members` frontier walk for only the requested sessions. Lifecycle events refresh one compact session summary and update the connection-local metadata/membership cache; family membership is recomputed in memory, not by rereading the database. Root activity still comes from the runtime/coordinator/status/permission/background-job authorities. Runtime-only branches and explicit execution-root hints remain supported. Parent-chain root resolution can still issue canonical ancestor point reads. A sessionless permission reply deliberately retains the authorized-baseline refresh fallback.

The conversation-search worker processes at most 16 documents / 2 MiB of source JSON per batch (one oversized document can run alone), with one native UPSERT statement per batch. Productive ticks run on a 100 ms delay and idle probes on a 1 s delay. Dirty-queue and migration probes are small indexed reads; no content search is required for pending backfill/projection work. The previous per-statement FTS flush fix remains intact. No new schema or migration is introduced here.

## Other database consumers reviewed

Full canonical transcript reads still exist where history is actually consumed: prompt/context preparation, compaction, undo/replay, transfers, and external-agent import/recovery. External-provider catalog matching also still enumerates canonical sessions because compact public summaries deliberately omit opaque external-session identifiers. Interrupted nested external-session reconciliation enumerates sessions during recovery, not as a catalog metadata-event subscription. These remain candidates for separately measured work rather than replacing their richer data contracts with incomplete metadata.

## Safety and verification

No rendering/animation settings, user database files, cleanup commands, schema rebuilds, vacuuming or destructive migrations are involved. Tests use separate temporary databases. Regression coverage checks family/override parity, compact baseline reads, single-session create/update/delete refresh with poisoned unrelated canonical/summary rows, stale event payloads, empty/single-row page isolation, title-message position ordering, expression/session index query plans, and unchanged-index maintenance suppression/error cooldown.

The previously observed 19.2 GB logical-read burst has not been attributed to a specific live SQL statement. These are confirmed source-level amplifiers, not proof of the entire runtime cause. Validate the rebuilt daemon with before/after process/thread I/O sampling under the same workload; distinguish startup backfill/first idle merge from unchanged steady state. The large live database was not queried during this investigation.
