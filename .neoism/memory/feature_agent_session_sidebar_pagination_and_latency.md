---
name: "Agent session sidebar pagination and latency"
description: "Session sidebar now loads compact pages and paginates on scroll"
type: "feature"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Fixed Agent session sidebar initial-load latency and missing pagination. Root causes: clients discarded `/v2/sessions` `cursor.next`; all scroll paths only clamped loaded rows; continuation results would have replaced/reset the list; server sidecar query still joined/deserialized full multi-MB `sessions.info_json`; desktop withheld first page behind serial `/v2/sessions/status`. Shared SidePanel now stores next/requested cursors, single-flights near-tail loads, appends+dedes by ID without resetting scroll, invalidates on directory change, and retries initial errors after 750ms. Desktop and wasm wheel/touch route through shared pagination behavior; protocol/daemon echo requested cursor to reject stale overlap. Server sidecar has compact summary_json for new/mutated rows and reconstructs legacy summaries from sidecar columns without canonical JSON joins; incomplete backfill never returns final-looking partial 200. Desktop publishes list immediately without status wait. Tests cover append/dedupe, one-shot tail request, retry loader, daemon URL cursor, and server keyset paging. Full cargo check passes including wasm. Workspace-daemon full test target has unrelated pre-existing AppState lsp_runtime initializer failures; targeted --lib daemon test passes.
