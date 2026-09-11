---
name: "TS GUI chat pin and anchored actions menu"
description: "TS GUI pin+ellipsis menu shipped; native server pin API authoritative, bounded cached-ID hydration, 580 tests"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-11"
updated: "2026-09-11"
---

Implemented in neoism-agent/sdk/typescript/packages/gui: Navigation uses ChatRow.tsx + dedicated chat-menu.css, outlined pin and ellipsis sibling buttons; pinned visible, otherwise hover/focus; portal right-anchored menu Pin/Unpin/Rename/Delete, arrow/Home/End navigation, Escape focus restore, outside/scroll/resize/unmount close. Inline rename form; guarded controller mutations; confirm delete. No copy-link/open duplicates or unsupported archive.
Authoritative backend exists: SDK client.sessions.pin(id,bool) -> POST /v2/sessions/{id}/pin, server session_routes.rs::session_set_pin stores flattened SessionInfo.extra['pinned'] in session info_json and publishes session.updated. Native unpin REMOVES pinned key, not false; mergeSession explicitly clears inherited pinned key for incoming unpinned full Session metadata.
No pinned-only list endpoint. sessionPins.ts SessionPinIndex is a browser per-server discovery cache ONLY of IDs (sanitized server key; no credentials/titles). Server remains truth. hydrateSessionPins uses scoped client.sessions.get, <=4 workers, cancellation predicate, 404 prune, retains 403/network failures, ignores empty/mismatched results. Unknown old pins made by other clients discovered via page/events, not magically exhaustive across all history. No full-history fetch. Controller merges hydration with pages/live mutation tombstones, filters roots/directory/title and pinned-first sorting/deduplication in recentSessions. Old callbacks + pending results guarded on client/token/server/list scope; deletion prevents late rename/pin resurrection.
Verification: npx tsc --noEmit -p packages/gui/tsconfig.json clean; npm test -w @neoism/gui: 57 files, 580 tests passed. 17 added tests across controller/sessionPins/ChatRow. No browser/Playwright, release build/restart, or global chat-width/gutter changes. GUI directory was entirely untracked before work; unrelated root changes pre-existed.
