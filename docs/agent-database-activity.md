# Agent database activity audit

## Confirmed source-level amplification

- `catalog_activity::project` used to call `list_sessions()` for every response, including an empty or single-row root page. That query selected every canonical `info_json`, including potentially large saved context/tool snapshots, and hydrated all hosting associations. Non-root listing already loaded all sessions before calling this projection, so it performed the enumeration twice.
- `CatalogProjection::project_event` repeated the same global canonical read for every session create/update/delete event, per catalog SSE subscriber. Metadata updates therefore amplified reads independently of content searches.
- Catalog SSE startup/reconnect previously loaded all canonical session JSON. It now uses existing compact list summaries once their persistent backfill is complete. Execution-root overrides are recovered with an expression-index range query; legacy databases retain the canonical fallback until backfill completes.
- Untitled external-session title repair loaded and deserialized the entire transcript to find its first user message. It now selects only the first user message in session-position order, with `LIMIT 1`. This can still examine preceding non-user rows; it does not return/decode trailing history.
- Search maintenance could run `OPTIMIZE INDEX conversation_search_fts` every 15 minutes even with no new projection work. It now performs the existing one-time idle maintenance for legacy segments, then requires productive projection work before another successful maintenance run. Idle settling, writer/search admission, retry backoff and cancellation boundaries are unchanged.

## Current bounds and retained behavior

Catalog pages use the existing indexed `catalog_family_members` frontier walk for only the requested sessions. Lifecycle events refresh one compact session summary and update the connection-local metadata/membership cache; family membership is recomputed in memory, not by rereading the database. Root activity still comes from the runtime/coordinator/status/permission/background-job authorities. Runtime-only branches and explicit execution-root hints remain supported. Parent-chain root resolution can still issue canonical ancestor point reads. A sessionless permission reply deliberately retains the authorized-baseline refresh fallback.

The conversation-search worker processes at most 16 documents / 2 MiB of source JSON per batch (one oversized document can run alone), with one native UPSERT statement per batch. Productive ticks run on a 100 ms delay and idle probes on a 1 s delay. Dirty-queue and migration probes are small indexed reads; no content search is required for pending backfill/projection work. The previous per-statement FTS flush fix remains intact. The official Turso upgrade described below adds a one-time derived-index migration.

## Scoped task and interaction reads

Child-session responses, pending subagent-completion checks, and task-tree inspection now seek the canonical parent expression index rather than call `list_sessions()` and discard unrelated sessions afterward. These consumers retain full canonical fields for authorization and completion records, but only for the requested family. Task traversal tracks visited IDs so malformed parent cycles cannot duplicate the root or loop indefinitely. Regression fixtures include an unrelated invalid canonical session and an unavailable list sidecar: neither may gate a family-scoped operation.

Permission and question polling snapshot only matching pending requests, release their map locks, and authorize the distinct represented sessions through canonical point reads. Empty snapshots perform no session authorization reads. Missing or unreadable sessions fail closed for authenticated callers; absent claims retain the existing local behavior.

## Unchanged search projections

Canonical session metadata and message changes outside the bounded searchable body can enqueue work without changing any projected search field. The worker compares the existing projection before preparing native FTS mutations. An unchanged projection emits only dirty-token cleanup, transactionally guarded by the revision, canonical source, and all current projected fields. It never opens an FTS write cursor through an otherwise empty UPSERT. Genuine content changes retain the existing guarded multi-document UPSERT and eventual search behavior. Dirty-only cleanup does not re-arm native index optimization; maintenance requires actual projection DML.

The dependency is official crates.io Turso `0.8.3`, with no local engine patch or vendored fork. Upstream replaces the old full-directory FTS loader with a segment-registry format. Its native FTS format is incompatible with 0.6.1 backing contents, so startup must transactionally drop/recreate only `conversation_search_fts`, persist a one-time migration marker, and finish before search workers start. Canonical sessions, messages, events, projection documents, and backfill cursors are retained. Old-format cold-open, rebuild, genuine-update, rollback, and reopen behavior must pass isolated fixture validation before activation; source inspection alone is not proof of workload improvement.

## Durable child-completion recovery

A persisted completion notification can survive without a successful parent reply. A fresh queue admission for that exact pending child-outbox record now attaches the existing user message as a saved-message recovery token before generation initialization. The store guards admission identity, session, delivery, request bytes, and existing message; it does not append another user turn or change queue order. Altered notifications and already-consumed duplicates cannot authorize recovery. A later initialization failure retains the token for retry. Startup also preserves durable deferred-completion state when optional runtime/tracker state is unavailable. Focused lifecycle and queue tests must pass before activation.

## Other database consumers reviewed

Full canonical transcript reads still exist where history is actually consumed: prompt/context preparation, compaction, undo/replay, transfers, and external-agent import/recovery. External-provider catalog matching also still enumerates canonical sessions because compact public summaries deliberately omit opaque external-session identifiers. Interrupted nested external-session reconciliation enumerates sessions during recovery, not as a catalog metadata-event subscription. These remain candidates for separately measured work rather than replacing their richer data contracts with incomplete metadata.

## Event-retention safety boundary

The raw event journal is not the authority for completed conversations, undo, exports, queued prompts, or subagent continuation, but `/v2/events` still promises cursor catch-up over its retained payloads. Native tail-only subscriptions use atomic in-memory unfinished-message/family baselines; SDK and ACP clients can instead reconnect with `Last-Event-ID`. Do not disable journal writes or prune old rows without durable global sequence high-water metadata, an explicit replay floor, and compatible snapshot reconciliation for expired cursors. Sequence gaps alone do not indicate expiry because transient events also consume wire sequence numbers. Preserve child click-through, current-prefix reconstruction, completion-during-replay ordering, pending interactions, and older hosted-client behavior. No event-retention policy or production cleanup is enabled by this change.

## Lossless event storage

The candidate retains every event and original sequence. New writes and a resumable background pass compress eligible JSON into a versioned BLOB envelope containing an eight-byte magic, codec version, exact uncompressed length, and checksummed zlib stream. Legacy TEXT remains readable. Only 2 KiB through 64 MiB payloads with at least 12.5% framed savings are compressed; other payloads remain TEXT. Legacy bytes are compressed without parsing/reserializing unknown fields. Corrupt, truncated, oversized, unknown-version, or trailing-data envelopes fail replay instead of silently skipping events.

The background pass seeks sequence IDs and point-reads payloads, with 128-row / 2 MiB source batches and one oversized eligible row alone. Compression occurs outside the writer gate; large compression and replay decoding use the blocking pool. Maintenance uses nonblocking writer admission and one busy attempt. Original-payload guards and migration progress commit together. Cancellation finishes only the in-flight bounded work. This reduces stored duplication, not historical retention: no rows are removed and total history can still grow. Freed database pages can be reused; physical file shrink is not promised.

Replay pages retain at most 128 rows and budget both stored and declared decoded bytes to 8 MiB, allowing one oversized first event. Byte-limited short pages do not terminate catch-up; only an empty page does. Tenant/session scope is revalidated on each point read. Wire payloads remain ordinary JSON. Atomic unfinished-message baselines and ordered live delivery preserve switching between active parent/child chats without missing prefixes or doubled token tails.

Persisted compressed BLOBs are not readable by older TEXT-only backend binaries. Rollback requires a pre-upgrade consistent backup or a separately verified lossless offline decompression migration; do not roll back a binary against migrated data blindly. Neither a live cleanup nor a backend restart has been performed during development.

## Safety and verification

No rendering/animation settings or live user database files have been changed during development. The candidate includes the derived FTS rebuild and lossless journal storage migration described above, but no event deletion, vacuum, or forced checkpoint. Tests use separate temporary databases. Regression coverage checks family/override parity, compact baseline reads, single-session create/update/delete refresh with poisoned unrelated canonical/summary rows, stale event payloads, empty/single-row page isolation, title-message position ordering, expression/session index query plans, and unchanged-index maintenance suppression/error cooldown. New engine/codec coverage must be verified separately before activation.

The previously observed 19.2 GB logical-read burst has not been attributed to a specific live SQL statement. These are confirmed source-level amplifiers, not proof of the entire runtime cause. Validate the rebuilt daemon with before/after process/thread I/O sampling under the same workload; distinguish startup backfill/first idle merge from unchanged steady state. No production database mutations are part of these fixes.
