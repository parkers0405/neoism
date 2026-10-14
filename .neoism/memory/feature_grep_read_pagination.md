---
name: "Native grep/read real pagination and reliability"
description: "Hard within-file grep cursors + bounded recoverable long-line reads, exact-mode/filter/metadata fixes; 53 focused tests + cargo check pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

# Native grep/read pagination and reliability

Implemented in neoism-agent-workspace-search-fff/src/lib.rs and agent-server tool_support/{workspace_search,file,read_pagination}.rs, tool.rs, tool_registry.rs, service-api contracts.

## Grep

Public `cursor` string / metadata `nextCursor` is real hard-match-limit continuation inside files. FFF retains candidate discovery; bounded streaming matcher does content scanning with before/after context and UTF-8 byte anchors. Context not counted as matches, maximum context=100. Normal page conservative budget 44k bytes/1800 rows; oversized single records use central artifacts; existing 4000-byte grep display-line clipping remains. Cursors are random 256-bit instance-held capabilities, 4096 cache, five-minute idle expiry, bound to query/options and checkpoint file identity. Indexed candidate path snapshots bounded by picker capacity; not content snapshots. Internal include/exclude remain workspace-root-relative; external scope-relative with absolute output paths. Explicit plain/regex never fuzzy-retry; oversized fuzzy errors rather than downgrades. Public pattern arrays remain literal alternatives. FFF definition classification via `definitions` feature and neo_frizbee same-version scoring preserved and compared with upstream in tests. Initial empty/nonsearchable index falls back; exact miss after searched files does not.

## Read

Opaque `cursor` supports UTF-8-safe continuation inside arbitrary long lines; no silent 2000-char clipping. `filePath` unchanged, omit `offset`; metadata continuation has usable full args. Bounded chunk buffers; paths/version/identity validated; cursors process-local random capabilities, 4096 LRU. `truncated` means remaining content; start/next byte/line/column and partialLine exposed. Content budgets reserve output framing/cursor and nearby instruction bytes/rows below central 51200/2000 limiter. Oversized instruction bundle retained for central artifact spill. Directory pagination remains offset-based. Blocking worker with cancellation/drop stop flag; bounded media reads and regular-file checks. Deleted now-unused paths::truncate_line.

## Verification

27 workspace-search-fff tests + 26 focused server read/schema/search integration tests passed (53 total); cargo check -p neoism-agent-server -p neoism-agent-workspace-search-fff passed. env -u CI required for fff-search. Broad server tool suite: 78 passed, 7 failures outside task in plugin-generation LSP/skills, diagnostics metadata, bash cancellation assertion. Earlier broad read_ filter also hit hosting 404-vs-200 failure. These areas untouched. No release build or commit. Tests must explicitly grant read permission in ToolContext; default requests require permission. Changes need rebuilt/restarted server before live tool schemas gain cursor.
