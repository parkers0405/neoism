---
name: feature-semantic-sidebar-and-win-hardening
description: Sidebar session search shows semantic excerpt chunks; serve plugins made Windows-safe (batch shims via cmd /C)
metadata: 
  node_type: memory
  type: project
  originSessionId: 39912326-ef2c-4184-a6b4-eec06c374bd3
  modified: 2026-08-27T01:36:22.307Z
---

2026-08-26 (commits c16bf60eb + 5870eb1b7):

**Windows hardening (c16bf60eb):** Windows CreateProcess can't exec .cmd/.bat; `build_plugin_command` now PATHEXT-resolves and routes batch shims through `cmd /C` via host-tested helpers `is_batch_shim`/`batch_shim_command` (plugin.rs, cfg(windows) glue only); serve-plugin npm install goes through the same builder (was raw `Command::new("npm")` — broken on Windows); publish.mjs picks npm.cmd on win32. Cross-compile from Linux blocked by ring/zstd C build scripts — Windows/mac CI legs are the compile gates. Mac note: bwrap sandbox is Linux-only; serve plugins run unsandboxed under Auto there, Required bails.

**Semantic sidebar search (5870eb1b7):** the sidebar search box already kicked `/v2/plugins/dev.neoism.semantic/search` per keystroke (coalesced in `kick_semantic_session_search`), but the side panel discarded excerpts (kept only ids to widen the title filter; the /sessions picker showed them in a "Related" section). Now: `NeoismAgentSemanticMatch {session_id, excerpt, distance}` in state/side_panel.rs; `rebuild_session_display` injects each session's best chunk as an `is_excerpt` ROW after the session row (fixed-height virtualized list stays uniform — same pattern as injected date headers); excerpt row id = parent session so hover/click/Enter resume it; `compact_excerpt` collapses whitespace (480-char ceiling); 979e9ff30: chunks WORD-WRAP into ≤3 is_excerpt rows — draw measures monospace columns per frame and feeds `set_result_wrap_columns` back to state (rebuild only on change); continuous accent tick across the run. b26cf2e00 root causes of "search only matches titles": (1) plugin-dispatched routes deliver query params as STRINGS — SemanticSearchQuery limit:Option<usize> rejected "20" → 500 on EVERY sidebar search, client swallowed as no-hits (usize_or_string deserializer; same hazard exists for any numeric query field on plugin routes); (2) embeddings need an API key (AuthInfo::Api) — codex OAuth cannot; route now ALWAYS runs store.search_messages keyword search (AND-terms, like_excerpt windows, >>markers<< stripped, distance 0.0) and blends semantic behind it when configured; available:false removed. Backend: hits come from `message_embeddings` (turso vector) via semantic.rs; `available:false` → client latches unavailable. Server excerpt = built by `search_document`.

**Term highlighting + OR recall (ef090c649):** excerpt rows carry `highlights: Vec<(usize,usize)>` byte ranges (computed in `rebuild_session_display` via `term_highlight_ranges` — ASCII-lower scan so offsets stay valid, sorted+merged); draw.rs renders segment-by-segment with `measure_text_cached` advances, bright fg 0.95 + cyan 0.16 wash behind matched spans; falls back to flat draw when truncate_to_fit shortened the label (offsets no longer align). Server keyword_hits: AND search empty + ≥2 terms → per-term retry merged by recency (fixes "tokenizer unicode" finding nothing).

STILL OPEN: web/wasm parity (shared state+draw are ready; wasm fetch wiring missing); typewriter reveal idea was REJECTED by user (started it misreading "keep going" — reverted, don't resurrect unasked).

Related: [[feature-sdk-ecosystem-wave]], [[feature-semantic-memory-recall]]
