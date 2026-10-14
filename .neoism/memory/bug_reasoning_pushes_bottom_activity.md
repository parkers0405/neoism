---
name: "Thinking sections push bottom activity: attempted fix reverted"
description: "REVERTED: user rejected always-docked activity during manual scroll; original policy restored; reasoning-growth complaint unresolved"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

REVERTED October 9 2026 at user's explicit request. The attempted native activity fix always pinned Crafting/Pondering above input even when manually browsing. User rejected this behavior and instructed rollback. Restored previous shared view/timeline/render.rs activity_geometry signature and policy: pin only when existing follow latch is true; otherwise activity remains transcript-tail attached and scrolls normally. Restored all prior tests/calls, removed own new streaming docking regression. Earlier Codex reasoning normalization/display fixes and other agents' unrelated changes preserved. Do NOT reintroduce always-fixed composer-dock status during manual scrolling. Original report (thinking growth differs from ordinary streaming text) remains unresolved; investigate the exact reasoning-specific transition without changing existing activity scroll behavior. No app restart, release build, or unrelated revert. Original attempted patch had 50 tests and native/shared check passing, but that does NOT validate user intent; rolled back nonetheless.
