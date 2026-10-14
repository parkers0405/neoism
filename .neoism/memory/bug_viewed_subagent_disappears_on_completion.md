---
name: "Viewed subagent return navigation"
description: "Viewed completed child kept for main-session return navigation, including old Task-click destinations; other historical children stay hidden"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

The old live-only exception was insufficient once Task rows could open completed subagent conversations: physically-pruned children lost sidebar family membership, so the Agents section disappeared and left no main-session return target. The shared side_panel.rs policy now retains the CURRENTLY VIEWED non-root child independent of lifecycle status. This is a visibility/navigation exception, not an active transition. set_subagents re-evaluates the hold when a roster arrives after navigation, preserves the viewed row when recovery omits it, and authoritative branch pruning preserves its row/activity. set_viewed_session_id clears the old hold and prunes eligible historical children when navigating away; nonviewed historical siblings stay filtered. Both shared state/hit_rects.rs and desktop pane/render_state.rs Task-click paths restore root/main and the clicked child BEFORE switch_session, pin viewed child before cache activation, preserve known lifecycle activity/runtime status, and use terminal recovery defaults only when status is missing (error => Stopped, otherwise Completed; stale running Task text is never authority). Regression tests cover completed/stopped rows, view-before-roster/roster-before-view, physically-pruned children with cold/cached navigation, empty authoritative and root-only recovery snapshots, returning via sidebar to root, zero active contribution for completed children, missing-main restoration and preservation of live activity/runtime. Native production cargo check -p neoism-ui -p neoism passed. This supersedes the prior rule that only children observed live could be retained: explicitly viewed historical children are now intentionally retained until leaving.
