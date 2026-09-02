---
name: feature-semantic-memory-recall
description: "memory.recall MCP upgraded to turso vector semantic search (2026-07-16) — memory_embeddings table in agent store, recall-time inline sync, keyword fallback"
metadata: 
  node_type: memory
  type: project
  originSessionId: da0c8dd7-b576-4f8f-b6f4-8cca272e97b1
---

`memory.recall` (builtin neoism-memory MCP) now ranks semantically. Shipped 2026-07-16.

**Architecture:** memory notes stay as markdown in notes vaults (`~/Neoism/Vaults/<vault>/Memory`, `Default/Personal/Memory`); embeddings live in a new `memory_embeddings` table in the AGENT store (`~/.local/state/neoism/agent.turso.db`), NOT the notes DB — the notes graph DB (neoism-workspace-index) has no vector columns and its FTS5 is sqlite-only.

- state.rs: `migrate_memory_semantic` (table keyed by absolute path + content_hash + model), `memory_embedding_hashes` / `upsert_memory_embedding` (`vector32(?)`) / `delete_memory_embedding` / `memory_semantic_search` (`vector_distance_cos`, root IN (...) scan).
- mcp_memory.rs: `call_tool_with_app_state` (async wrapper; the old sync `call_tool` is unchanged so tests keep compiling) → `semantic_recall` → `sync_memory_embeddings` runs INLINE at recall time (memory stores are tens of files): sha256 content-hash diff, embed changed files in batches of 16 (8k-char cap), prune deleted.
- mcp.rs dispatch passes `state.as_ref()` (the seam that previously dropped AppState).
- Fallback chain: no EmbeddingsClient (`state.inner.semantic`, env `NEOISM_AGENT_EMBEDDINGS_MODEL/_API_KEY`, default openai/text-embedding-3-small) OR non-turso store OR any error → old tokenized keyword scan. `model_spec` stored per row so a model switch re-embeds.
- System-prompt memory-index blurb updated (was "single-keyword substring").

Related: [[workflow-vault-memory-primary]] (recall was tokenized OR-matching), [[project-notes-overhaul]].
