---
name: "Conversation search scope + false HTTP 500"
description: "Search mirrors sidebar scope; batch canonical hydration; typed budget errors survive plugin dispatch; backend/web/WASM verified"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Conversation search scope and false HTTP 500

User reported `firecrawl` in Search showing a truncated `Neoism Agent HTTP 500 Internal Server Error: Conversati...` screenshot. Exact failing branch was not observable from screenshot; no live archive replay performed. Confirmed search admission/deadline/frontier limits previously used plain anyhow strings, becoming generic500, and semantic plugin adapter erased ApiError statuses via PluginRuntimeError. Search is local Turso FTS, not a Firecrawl/provider request.

Search must mirror sidebar `catalog_directory()`: dashboard/home/filesystem root -> None/all authorized conversations, declared workspace -> exact directory. Never derive from terminal cwd or selected chat. Desktop search identity includes directory for blank listing/transcript search and stale-reply invalidation. Shared Chrome exposes sidebar directory through WASM ChromeBridge/TerminalAdapter. Web SearchThreads.directory is optional/defaulted through protocol/daemon; scope changes invalidate stale replies.

Backend `state/conversation_search.rs::cache_canonical` batches 200 IDs, caches misses, and avoids four reads/full hosted namespace scan per session. Canonical tenant SQL, trusted hosting overlays, caller authorization, root grouping and bounded relevance proof remain. Preserve native CANDIDATES FTS shape: adding JOIN/WHERE may lose TopDocs pushdown. Limits remain; no silent partial-results fallback.

`error.rs::ConversationSearchError`: Busy ->429/search.busy/retryable; Deadline ->422/search.deadline; TooBroad ->422/search.too_broad. Semantic plugin adapter returns structured API response rather than runtime-error flattening. Real SQL faults remain500. Desktop http_error and daemon GET bridge expose actionable message only for recognized search codes/statuses; real faults keep HTTP context. State debug logging includes failure reason and directory.

Verified: 38 server search tests, server cargo check --tests, daemon error-display test, full web suite + TypeScript check, 34 focused web navigator tests, 12 protocol tests, 9 shared navigator tests, git diff --check, and WASM cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown all PASSED. Final desktop `cargo check -p neoism --tests` blocked by unrelated concurrent daemon change: missing agent_proxy_client in neoism-workspace-daemon/src/server.rs:383. Earlier attempts caught transient missing remove_session_metadata/retry_session_catalog in concurrent side_panel.rs edits (later fixed). Do not modify/revert those agents' changes.
