---
name: "Native agent chat static-work performance pass"
description: "Native agent chat: borrowed width-cache hits, static orb geometry with bit-exact animation, wrap cache; baseline logs and verification limits"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

Native Rust agent chat performance pass. Changes: view/draw.rs TextMeasureCache now nested style -> String map, borrowed &str lookup means zero text-key allocation on hits; bounded FIFO style buckets removed when empty. presence_avatar.rs circular_geometry OnceLock caches coordinates/dist/rim for grids 11..15; boundaries shared per draw; all time/sine calculations remain live and bit-exact. Grid 15 is possible because u32 -> f32 RNG conversion can round to 1.0, despite original comment 11..14. Tests compare 8,960 parameter cases bit-for-bit and ensure animation colors change. Composer/user-bubble wrap caching in view/user_input/wrap_cache.rs implemented separately in same pass, final verification pending at write time.
Ran existing target/debug/neoism in separate process with NEOISM_AGENT_UI_PERF_LOG=1 RUST_LOG=warn,neoism::agent_ui_perf=info and isolated daemon/IPC paths under /tmp/neoism-agent-perf-runtime. Log /tmp/neoism-agent-ui-perf.log. Existing binary baseline only (not rebuilt changes): 419-char unchanged draft sampled pane CPU p50 716us p95 1095us; does NOT include full GPU/frame cost. Isolated daemon never hydrated history, so no representative long-chat/streaming measurement. Diagnostic PID 2525878 terminated; user's existing desktop processes left running. perf/samply not installed. No release build performed.
Verification: env -u CI cargo check -p neoism --features wgpu passes. cargo test -p neoism-ui --lib panels::agent_pane: 647 pass, 4 state failures outside edited paths (clicking_the_active_detail_child_returns_to_main, mobile_child_and_root_navigation_both_dismiss_takeover, mobile_semantic_search_result_navigation_dismisses_takeover, tab_mode_switch_rearms_agent_chip_transition). Final presence_avatar tests 7 pass, draw::measure_tests 5 pass. Logs /tmp/neoism-agent-perf-{check,tests,avatar-tests,measure-tests}.log.
Remaining investigation from Explore: timeline/layout.rs mark_changed_artifact_rows scans all cached rows per reusable frame, with text equality/contains comparisons; synchronous API/config operations during desktop ingest can stall UI; markdown rendering walks preceding blocks for deep viewport. These are unmeasured leads, not proven bottleneck rankings. Preserve continuous user_orb animation_reason; do not disable animations to claim speed gains. Several related files have concurrent user/agent changes: never revert them.
