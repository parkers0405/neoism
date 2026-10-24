---
name: "Native activity initial bottom jump"
description: "Native Crafting/Pondering begins under user for short transcripts; cap pinned Y at tail, preserve long-response and non-sticky browsing behavior"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

Native Rust agent activity placement lives in `neoism-frontend/shared/src/panels/agent_pane/view/timeline/render.rs::activity_geometry`. Its following branch used viewport-bottom positioning even for user-only/short transcripts, so Crafting/Pondering initially appeared across empty space above the composer. Fixed by capping the existing following Y with `.min(attached_y)`: short conversations start immediately after the visible transcript; long-response pinning, spring lag behavior, reserve/ink clipping, and browsing/offscreen non-sticky behavior are unchanged. Do not remove following behavior globally or change `timeline_follow_bottom`: user explicitly requested preserving scrolling behavior. Added tests in timeline/tests.rs for initial user-only placement, first delta and growth through viewport overflow at multiple scales/status line counts, and empty-transcript spacing; existing pinning, resize, and browsing tests retained.
