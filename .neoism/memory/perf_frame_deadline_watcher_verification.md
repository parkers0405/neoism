---
name: "Frame-drop pass final verification and search pin lifecycle"
description: "Frame-drop pass verified: final desktop check, Git3/router14/root6/FFF10 tests pass; production agent search pin lifetime completed"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

Completion supplement to perf_frame_deadline_watcher_loop.md. Production agent search pin lifetime is implemented in neoism-agent-server/src/workspace_runtime.rs (same injected AgentServices workspace_search instance, retained across plugin reloads, released after current+retired generation lease drain on successful disposal even if runtime Arc retained; failed disposal retains pin; RAII on final drop). Server tests.rs recording-service regression validates repeated root reuse, normal grep sees pin, no subpath pin, reload stability, lease-safe teardown and shutdown. Existing one-hour runtime idle policy unchanged. FFF engine10/10 pass. Retirement reviewed in installed FFF dependency: stop_background_monitor uses stop_nonblocking; no callback join while picker write lock held. Normal agent grep must have this production runtime pin; warm alone is not a pin and without wiring ordinary grep would regress to fresh full scans.
FINAL integration verification completed successfully after other workspace build jobs and in-progress usage/reveal edits settled: env -u CI cargo check -p neoism --features wgpu PASS (warnings only); cargo test -p neoism-ui --lib panels::git_branch 3 PASS. Logs /tmp/neoism-frame-final-check.log and /tmp/neoism-frame-git-tests.log. Earlier router:: tests14 PASS, workspace_panel_owner_tests6 PASS from compiled desktop binary, FFF10 PASS and server lifecycle/service-isolation tests PASS. No debug/release application binary build was performed, honoring user's own rebuild loop. All todo entries for this corrective pass complete. Still NO measured sustained144Hz before/after result from rebuilt app; current running processes cannot acquire source fixes without rebuilding/restarting. Do not conflate targeted pass completion with zero-lag guarantee.
