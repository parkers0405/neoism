---
name: "Agent late-join missing prefix: live baseline implemented"
description: "Atomic in-memory live baseline for late attach/reconnect; Rust/TS authoritative text and immediate family hydration; no token batching or per-token DB writes"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-17"
---

## Symptoms and causes
Late joining/switching workspaces during assistant streaming sometimes displayed only a suffix until final part update. Provider live deltas are deliberately non-durable; REST history has stale unfinished text. TS metadata-stream multiplexing reused an already-started future-only stream, bypassing any new attachment baseline. Native reconnect also blocked SSE reading on a duplicate statuses request. Native runtime hydration discarded authoritative family data on unrelated token epoch changes; TS subagent panel waited for all three requests.

## Implementation (2026-09-17)
- `neoism-agent-server/src/state/live_messages.rs`: transient projection of unfinished messages/parts plus active runtime families, reduced under existing broadcast-order Mutex. `subscribe_with_messages` atomically captures baseline and subscribes. Delta processing appends only new text, no per-token persistence. Message completion evicts bodies; runtime tombstones retain (execution ID, execution revision, family revision) to reject delayed stale active snapshots. Current wire field is familyRevision; older shipped field revision is accepted.
- `v2_routes.rs`: fresh-ID, cursorless full info/part/runtime events before post-baseline live events. Durable replay excludes message events superseded by the captured active-message baseline, including final edges committed during replay, so queued final copies are not incorrectly deduplicated after an older baseline.
- Rust desktop/shared stream upserts replace text authoritatively (including empty retries); REST reconciliation still preserves live text. Shared runtime event parser registers child IDs immediately. Desktop reconnect reader no longer performs synchronous status HTTP fetch; EventStreamReconnected already requests revision-guarded background hydration. Desktop family hydration is applied using family/execution revisions before the unrelated token epoch gate.
- TS useChat uses a baseline-bearing dedicated subscription, preserves active text through quiet REST refreshes, refreshes unfinished caches on return, and refreshes on visibility. Subagent controller renders runtime independently of slow task/child fetches and handles live runtime directly. `runtimeIsOlder` checks execution IDs/revisions then familyRevision (legacy revision fallback); no generated SDK edits.

## Verification
5 new server baseline tests passed (concurrent tokens exactly once, reconnect fresh IDs, retry/removal/completion, runtime revision, completion during replay). 555 shared Rust agent-pane tests passed. TS GUI suite passed 698 tests, and tsc passed after an explicit test parameter annotation. Full desktop check/test verification was blocked by concurrent computer_use/typesafe.rs compile errors (latest E0505 cancel borrow around line 195); do not modify/revert that unrelated work. No release build, commit or GUI restart.
