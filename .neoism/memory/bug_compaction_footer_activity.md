---
name: "Compacting footer activity lifecycle"
description: "Compacting footer vanished because manual summary runs lacked Busy and desktop/cache classifiers ignored Compaction; lifecycle and classification fixed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

User observed missing bottom `Compacting...` during manual `/compact`, while ordinary `Crafting...` streaming footer is visible. This is separate from the execution admission deadlock in [[bug_manual_compaction_admission_deadlock]]. Investigation found no special compaction footer geometry: shared timeline pins all activity above composer while following bottom; follow=false allows the footer after content to be offscreen. The compaction-specific confirmed code mismatch was activity ownership, not positioning.

Backend `session_context::compact_session_context` directly registers a coordinator/store run but formerly never inserted/published Busy; `/v2/sessions/status` only enumerates statuses map, so desktop status reconciliation could treat an actively summarizing session as idle and clear/latch activity. `release_owned_compaction_run` also lacked Idle publication. Fixed by extracting `session_run::register_session_busy`, reused by normal run startup and successfully admitted manual/owned-transient compaction; the helper does not acquire execution admission. Releasing compaction publishes idle through `publish_idle_if_no_run` only if the exact owned coordinator run was released, preserving parent/current/queued work. Added `SessionRunGuard` to manual/transient owned compaction, with disarming after cleanup including both admission failure branches, preventing dropped requests from leaving Busy wedged. Borrowed auto-compaction parent runs are not idled/released.

Frontend desktop `pane/session.rs::note_streaming_from_part` and `CachedAgentRuntime::refresh_streaming_from_tail` formerly treated Compaction as a no-op, so summary chunks could not restore activity. They now classify Compaction as Compacting. Applied matching changes to shared `state/streaming.rs` and `state/session_cache.rs`; shared live delta ingestion already mapped kind=compaction correctly. No footer layout, summary duplication, or historical transcript changes were needed.

Coverage: desktop live STARTED/part updates/deltas, recovery via busy runtime reconciliation, stable clock, ENDED settling, cached summary tail recovery; shared part/delta/tail/cache recovery and Ended; server Busy-before-Started, Busy during stub summary streaming, success/error/abort/drop and transient cleanup, exact-owner/parent release protections. Test targets compile, tests were NOT executed per user build-workflow preference. Final `cargo check -p neoism -p neoism-ui -p neoism-agent-server --tests` passed, and git diff --check passed. Existing warnings remain unrelated. No paid compaction retries, history mutations, binary builds, or runtime restart during this indicator fix. Requires user's regular desktop rebuild and daemon restart to take effect.
