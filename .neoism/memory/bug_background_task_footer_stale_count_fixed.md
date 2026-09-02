---
name: "Background task footer stale count fixed"
description: "Completed background jobs no longer leave Agent footer permanently saying tasks are running"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Fixed Agent GUI footer stuck on `N background tasks running` after jobs had completed. Root cause: displayed count read a cached integer refreshed only on selected live-event ingest paths; completion sentinels can arrive through history/snapshot merge, leaving transcript authoritative completed state but stale cached count forever. Both shared/web and desktop `running_background_task_count()` now derive from message lifecycle on read, where running starts are excluded by later completed/cancelled/error/timed_out cards or empty snapshots. Cache remains only for handoff/clock seeding. Added regression modeling completion added by snapshot without clock refresh. Test and cargo check for neoism-ui+neoism pass; no push/commit.
