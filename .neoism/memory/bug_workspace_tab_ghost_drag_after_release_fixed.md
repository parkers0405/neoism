---
name: "Workspace tab ghost drag after release fixed"
description: "Workspace tab could remain grabbed after mouse-up and later detach; fixed release priority and cancellation invariants"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Fixed intermittent workspace/Island tab ghost drag. Root cause: `Island::begin_drag` latches armed state on press; LMB release was handled only after palette/sidebar/editor release handlers that could return early, and cursor motion called `update_drag` without checking LMB. A consumed/missed release left drag live, later motion armed detach, and a later unrelated release threw workspace into new window. Fix: Island gets first refusal on every LMB release; sub-threshold release still clears and falls through. Cursor movement cancels Island if authoritative LMB state is not Pressed. Focus loss clears all button states and cancels Island without commit. A fresh LMB press cancels any stale Island gesture app-wide. Release delivered to another Neoism window finalizes the source owner. Added Screen `has_island_drag`/`cancel_island_drag`; shared Island cancellation tests prove armed/live/detach-armed cancellation cannot detach and release always clears. Files: desktop app/window_event/mouse.rs, focus.rs, screen/lifecycle/splash_island.rs; shared widgets/island/drag.rs and tests.rs. cargo check -p neoism passed; targeted 3 tests passed. No push/commit.
