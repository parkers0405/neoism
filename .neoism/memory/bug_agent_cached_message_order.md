---
name: "Agent cached message order on parent return"
description: "Agent GUI cached transcript ordering and subagent completion notices fixed with identity-anchored merge"
type: "bug"
scope: "project"
origin: "session fix after direct report"
created: "2026-08-21"
updated: "2026-08-21"
---

Fixed 2026-08-21. Reopening a parent/main agent conversation after viewing a child could reorder its transcript as `[newest fetched page] + [older cached prefix]`; live `Subagent finished...` runtime notices that landed in the parent cache were therefore moved to the bottom. Root cause was `merge_session_snapshot` in both desktop and shared panes: it emitted the server snapshot first and appended every unmatched cached row.

Fix: snapshot/cache reconciliation now treats shared durable message identities as chronological anchors. Cached rows before the fetched page remain a prefix; unmatched live rows between shared IDs stay in that interval; genuinely newer live rows remain after the snapshot. The desktop active paginated-history refresh now uses this same ordered merge instead of preserving only the prefix before the first overlap.

Files: `neoism-frontend/desktop/src/neoism/agent/pane.rs`, `pane/ingest.rs`, `pane/tests.rs`; shared parity in `neoism-frontend/shared/src/panels/agent_pane/state/session_cache.rs` and `state/tests.rs`.

Regression tests: `partial_snapshot_keeps_cached_history_and_subagent_notice_in_order`; `returning_from_child_keeps_parent_subagent_notice_in_place`. Focused desktop/shared ordering and completion-card tests pass; touched-crate rustfmt check passes.
