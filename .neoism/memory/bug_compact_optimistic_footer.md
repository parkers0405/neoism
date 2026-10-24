---
name: "Manual compact immediate queued footer - FIXED"
description: "FIXED missing immediate /compact footer feedback: separate optimistic request-scoped queue overlay, authoritative reconciliation, desktop/web/cache parity"
type: "bug"
scope: "project"
origin: "current session source fix and cargo checks"
created: "2026-10-18"
updated: "2026-10-18"
---

Follow-up to [[bug_manual_compaction_not_queued]]. User reported no bottom queued indicator for /compact during response. Confirmed local mismatch: steer updates queue fields immediately; manual compact_session only pushed outbound, and CompactAccepted deliberately no-op. Inspected current backend queued events/classifier/renderer path supports display; no evidence of filtering bug. Don't claim actual SSE delivery was exercised.

Fixed frontend-only: desktop/shared pane and cached runtime pending_compact:Option<String> request identity, set at local slash dispatch. status_policy::visible_queue_count/preview overlay pending feedback without mutating authoritative ordinary prompt fields. Repeated local pending commands coalesce; known authoritative compact preview avoids duplicate dispatch/count. Existing queue getters feed bottom footer so active response immediately shows queued compaction. Queue/status snapshots, true compaction started/ended, Stop/reset, and matching admission failure reconcile local pending state. CompactAccepted remains a non-authoritative no-op so late HTTP success cannot resurrect completed feedback. Ordinary prompt preview preserved when already queued.

Correlated outbound CompactSessionRequest{session_id,request_id} keeps original session if user switches before dispatch. Desktop async failure includes optional request_id; clears only matching current/cache feedback. Protocol AgentClientMessage::Compact adds optional request_id, TypeScript workspace mirror updated. Daemon emits SessionEvent kind session.compaction.admission.failed properties{requestID} plus origin-scoped Notice on failure. Wasm handles failure and queued events for active/cache; generic errors do NOT clear local compact feedback. Web hydration emits exact QueueUpdate even omitted/idle status without inventing Idle lifecycle edge.

Touched desktop commands/pane/session/ingest/tests, shared state/cache/streaming/timeline/outbound/status_policy/protocol_mapping/tests, wasm catalog, protocol+test, daemon dispatcher/handlers, web workspace type. No backend or auto-compaction changes. Added tests immediate dispatch during Generating, dedup, authority no double count, failure preserves ordinary queue, stale failure/new request, inactive-session cache, late success after clear, canonical SSE decode->match->classify. Tests TYPECHECKED, not executed, per user cargo-check-only workflow.

Verification passed cargo check -p neoism -p neoism-ui -p neoism-protocol --tests; daemon+wasm --lib --bins; actual wasm32-unknown-unknown check. First check caught desktop private fields; fixed by pub(super) new pending field and public preview getter rather than exposing old fields. Existing warnings unchanged. No app rebuild/restart or live GUI/provider calls; takes effect in user's normal dev build. If queued state stays absent for a whole long response despite rebuilt client, capture actual SSE+queuefields rather than assuming missing optimism explains lost server events.
