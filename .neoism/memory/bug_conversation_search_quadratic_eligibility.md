---
name: "Conversation search quadratic eligibility and hard deadline — FIXED"
description: "FIXED: 2s cutoff hid O(n²) eligibility scans; message-ID seeks + one FTS cursor cut dense fire fixture 11.4s to 269ms; idle compaction + latest-query workers"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Conversation search: remove hard deadline and quadratic canonical validation

The screenshot error for `fire` came from `SEARCH_TIMEOUT = 2s` wrapping `search_inner` in `neoism-agent/crates/neoism-agent-server/src/state/conversation_search.rs`. The dominant measured problem was NOT FTS or embeddings: a 1,024-message chat fixture spent ~57 ms in native FTS and ~11.32 seconds in eligibility SQL. EXPLAIN showed `SEARCH m USING INDEX idx_messages_tenant_session_position (tenant_id=? AND session_id=?)` inside the docs -> sessions -> messages correlated EXISTS. For every candidate it walked the chat's message history to find one message, producing quadratic work. Index compaction alone did not change the 11-second search time.

Fixed eligibility by looking up bounded message-ID batches directly: `SELECT m.id,m.session_id FROM messages m JOIN sessions s ON s.id=m.session_id AND s.tenant_id=m.tenant_id WHERE m.id IN (...)`. EXPLAIN now uses `sqlite_autoindex_messages_1 (id=?)`; the regression asserts that plan. Projection/source session identity is checked in Rust. `cache_canonical` still applies canonical tenant SQL, trusted hosting overlays, and caller authorization before counting hits; title projections also require authorized canonical sessions. Tests cover missing messages/sessions and mismatched canonical message tenant/session IDs.

Removed the wall-clock timeout and unused typed Deadline error, preserving process-wide admission (2), candidate/session/ancestor caps and explicit TooBroad failures rather than silent truncated recall. Search now keeps ONE bounded native TopDocs cursor (16,384 + one exhaustion lookahead), streams growing metadata pages without rerunning increasing prefixes, caches canonical records as Arc, memoizes per-source root resolution, batches ancestor hydration, and fetches all final excerpt payloads in one bounded query. Exact score-frontier/tie, grouping, directory and authorization rules remain. Stage timings/counters are logged under neoism_agent::search without transcript/query bodies.

Turso 0.6.1 uses NoMergePolicy. Added worker-owned `OPTIMIZE INDEX conversation_search_fts` after 30 seconds of projection idleness, at most every 15 minutes. Maintenance yields to active searches, pending indexing and an occupied writer gate. Native merge runs in spawn_blocking, owns the writer gate without re-locking it, and must finish once started. It is not on the foreground search path; no live user database was manually modified. Fixture tests cover optimization, subsequent indexing and persistence across reopen.

Desktop `screen/bridges/conversation_navigator.rs` now has two fixed workers and ONE replaceable pending request, keeping the 160 ms debounce. New input/cancel invalidates stale completions/retries; one slow obsolete HTTP fetch no longer blocks the newest request. True transport abort remains unavailable: if both synchronous fetches block, newest work waits for a slot. Render wake runs outside queue mutex and re-arms its flag on every completion. Eleven navigator tests passed; desktop cargo check --lib --tests passed.

Measured debug fixture search before/after: ~11.4 seconds -> ~269 ms for 1,024 dense messages plus two other matching documents, before any compaction. Compaction of that fixture took ~90 ms. These are fixture server timings, not a live-UI or production-corpus claim. Preserve primary-key eligibility and one native cursor; do not restore correlated message-existence checks or a hard 2-second deadline. Related: [[bug_conversation_search_scope_and_false500]], [[bug_turso_fts_statement_flush_churn]], [[feature_conversation_search_fts]].
