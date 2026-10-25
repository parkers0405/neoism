---
name: "Local conversation search: Turso FTS + ranked modal"
description: "Local Turso FTS chat search; revisioned background index, relevance/grouped snippets, rounded modal, bounded retrieval; no embeddings required"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

# Turso-backed conversation search

Conversation search is local lexical retrieval, not an embedding-provider call. `neoism-agent-server/src/state/conversation_search.rs` owns a Turso-native Tantivy FTS projection over title/message documents, a durable revisioned dirty queue, and resumable history backfill. `SessionStore::open` enables `experimental_index_method(true)`, migrates the projection, and owns one background worker per store. All canonical message/session mutation paths (including event-backed writes and imports/deletes) maintain the queue/projection transactionally. Streaming edits settle for 500 ms; worker batches are 16, with 100 ms productive/1 s idle polling. Live projections are skipped by backfill to avoid redundant FTS commits. Canonical history remains authoritative.

`state.rs::search_index` is shared by UI retrieval and strict-AND `session_search` tool retrieval. The compatibility endpoint remains `/v2/plugins/dev.neoism.semantic/search`; UI requests `conversation=true` (also accepts `conversations`). Response includes `available`, `indexing`, and relevance-ordered hits with `sessionId`, `messageId`, `excerpt`, `score`, and embedded authorized `session` metadata. Conversation mode uses OR/BM25, title boosts and user weighting, groups child hits into authorized root conversations before limiting, and returns selected match excerpts. Exact directory and tenant/hosting/revocation policy are enforced against canonical metadata before results count toward the limit.

CRITICAL Turso 0.6.1 optimizer constraint: indexed candidate SQL must retain the exact `fts_match` + `fts_score ORDER BY score DESC LIMIT` shape. Adding GROUP BY/JOIN or extra WHERE scope filters can lose native TopDocs LIMIT pushdown, invoking a path that allocates up to a million hits or scalar fallback with zero scores. Scope/stale-record prefiltering instead happens in a separate bounded-ID SQL stage (256 keys). Regression tests verify both QUERY INDEX METHOD and bytecode COMBINED_ORDERED_LIMIT pattern/limit parameter.

Search resources are bounded: two searches process-wide, two-second async budget, at most 16,384 candidate identities/cached sessions, 128 ancestry hops, and payload reads only for final top-K (at most 100). Relevance frontier is proved before returning; budget exhaustion reports an explicit refine-query error instead of silently dropping older matches. Document bodies are UTF-8-bounded to 64 KiB and completed tool outputs to 2,000 characters per output. No startup history scan and no FTS indexing in the renderer.

Frontend Search conversations entry sits below New chat and above Projects; typed header keyboard selection follows the same order. Shared `widgets/conversation_navigator.rs` uses existing rounded inline-picker styling, compact query-aware excerpts, and an indexing footer/empty state. Desktop and browser preserve backend rank and consume embedded session metadata instead of N+1 HTTP GETs. An open query refreshes once per second only while indexing, with same request identity and single-flight behavior; close/query/endpoint change, error, timeout, and disposal cancel future refreshes. Desktop reuses one coalescing worker with Query/Cancel commands; browser timers have explicit cleanup. Daemon browser bridge uses the same grouped route.

No embedding/model download was added. The search route never invokes an embeddings provider. Existing transcript embedding indexing now requires explicit `NEOISM_AGENT_EMBEDDINGS_MODEL`; merely possessing an OpenAI API key no longer opts the archive into paid indexing. Memory embedding services remain separate. Plugin ID/path stay compatible, but its display name is Conversation search and its route requires only WorkspaceRead, not provider/network/secret dependencies.

Useful verification: `cargo test -p neoism-agent-server --lib search -- --test-threads=1`, plus `transcript_fts_tracks_direct_and_event_writes_deletes_and_reimport`; `cargo check -p neoism-agent-server -p neoism -p neoism-workspace-daemon --tests`; `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web`. Includes strict-AND recall beyond 500 newer seed matches, plan/limit pushdown, scopes, grouping, title/snippet ranking, mutation races, background lifecycle, backpressure and zero-provider-request tests. Browser host tests also cover indexing refresh and cancellation.
