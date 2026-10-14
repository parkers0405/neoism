---
name: "Read-group expansion coupling - fixed"
description: "Read groups clipped after click: synthetic IDs bypassed animation dirtying; stable source/child IDs, final remeasure, and disjoint click targets fix it."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

# Read-group expansion coupling

Fixed the compact tool-row regression affecting synthetic Read/search groups. Group IDs were `first..last`, but layout animation dirtying examined original source message IDs only. A click rebuilt the group at near-collapsed animation height and subsequent frames reused it, clipping the expanded children. Shared and desktop `tool_expand_animating(source_id)` now also match the source-owned `source_id..` group prefix. Finished animations are removed in `take_timeline_dirty_marks` and mark their original source IDs dirty once, guaranteeing a final settled-height remeasure.

Groups now have stable `first_id..` IDs instead of changing whenever another read arrives. Each child uses its original tool-call ID in the synthetic message's tab-separated row and preview data; labels/status are purely presentational, avoiding duplicate-label preview collisions and loss of selection on status transitions. The existing grouping threshold and four-member preview cap remain unchanged.

The first child row starts at scaled y=36, so its click target begins below the 30px header. Header and child targets no longer overlap. Child clicks now set the scroll anchor and stop velocity before selecting the preview; numeric child IDs are excluded from diff-file target detection. Group header anchoring resolves to the first original source message. Compact styling and expansion animation are preserved.

Files: shared `state/hit_rects.rs`, `state/caches.rs`, `view/timeline/read_group.rs`, `view/tool_message/render.rs`; desktop equivalents in `neoism/agent/pane/render_state.rs`. Regression tests cover source dirtying, animation completion, stable IDs on append/status changes, duplicate labels, header/child separation, numeric child targets and scroll anchoring. Six new read-group tests, 37 timeline tests and existing tool/diff regressions passed. Desktop binary test compilation passed. Broad shared integration-test compilation has an unrelated `host_git_panel` fixture missing GitDiffIo fetch/pull/push methods.
