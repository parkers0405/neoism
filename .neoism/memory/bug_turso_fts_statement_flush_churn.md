---
name: "Turso FTS per-statement merge churn — FIXED"
description: "FIXED: per-document/statement Turso FTS commits caused minutes of merge CPU; one multi-row UPSERT per 16-doc/2MiB batch reduced old501 test to 8.49s"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

# Turso FTS flushes on statement cursor drop: batch one SQL statement

Turso 0.6.1's Tantivy `FtsCursor::drop` flushes the native index when a statement cursor is dropped, even inside an enclosing SQL transaction. A transaction containing sixteen separate message-document UPSERTs still produces sixteen native commits and excessive segment merging. The conversation-search >500-history regression spent roughly four minutes in indexing with ~63% CPU in the test thread and ~60% in the segment updater before it was stopped.

Fixed in `neoism-agent/crates/neoism-agent-server/src/state/conversation_search.rs`: `projection_statements` retains per-document queue nonce and canonical-JSON guards; `combine_projection_statements` combines inserts into one `INSERT ... SELECT ... UNION ALL ... ON CONFLICT ...` statement, consolidates invalid-document deletes, then acknowledges revisions in the same transaction. Worker batches are at most 16 documents and 2 MiB of accumulated raw canonical JSON. An oversized document runs alone. Prepared work may be discarded on cancellation, but an active transaction is never aborted. Backfill also skips documents already projected by live updates.

After the fix, the exact old501 regression passed in 8.49 seconds including fixture setup/indexing/query, and all 13 module tests passed in 9.85 seconds. Tests verify byte ceilings, oversized progress, revision/deletion races, and actual native FTS rollback across fresh connections and reopening. Preserve the one-native-UPSERT-per-batch property; merely putting independent UPSERTs in one transaction is NOT equivalent. See `feature_conversation_search_fts.md` for retrieval resource limits and UI architecture.
