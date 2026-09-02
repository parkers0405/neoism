---
name: "Scheduled workflow atomic occurrence claim — FIXED"
description: "Scheduled workflow claiming and cursor advancement are atomic across SQLite and Turso, with crash/restart idempotency coverage."
type: "bug"
scope: "project"
origin: "release-blocking scheduler no-repeat guarantee"
created: "2026-08-24"
updated: "2026-08-24"
---

Release-blocking scheduler race fixed in `neoism-agent-server`: scheduled occurrence insertion into `workflow_runs` and monotonic `workflows.last_scheduled_at` advancement now happen in one backend-neutral transaction (`claim_scheduled_workflow_run`) for both sqlx SQLite and Turso. Manual runs still use the old standalone claim; overlap suppression still advances the cursor, preserving coalescing. Restart regression reopens both backends immediately after a queued claim, marks recovery interrupted, retries the same slot, and proves only one durable run exists. Focused workflow suite: 12 passed; cargo check, rustfmt check, and git diff check pass.
