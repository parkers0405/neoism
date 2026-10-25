---
name: "Chat-open follow-up: SSE and Turso index barriers"
description: "PARTIAL: SSE pre-header catalogue scan + Turso IN index miss fixed; debug build98native25backend pass; updated GUI trace awaits restart permission"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Follow-up: saved-chat transcript still takes 1-3 seconds

The user rejected declaring the earlier chat-open work complete and explicitly requested cargo build, runtime debugging, and self-testing. This increment is PARTIAL: two additional proven backend barriers were fixed and the final debug binary built, but updated GUI timing awaits permission to restart the user's existing debug window with logging enabled. Do not claim the 1-3 second GUI delay is resolved or that the 350 ms native benchmark measures paint.

## Runtime observations before these fixes

Use the user's existing debug window on Hyprland workspace 3, not another launch. At this run it was PID 214390, embedded daemon214428, agentHTTP http://127.0.0.1:45541. These identifiers are ephemeral. The original production GUI/4096 backend is a DIFFERENT dataset. I mistakenly launched duplicate debug GUIs; the user closed them and explicitly said stop. They must not be relaunched. Permission was requested in commentary to restart ONLY the existing workspace3 debug with logging, but no affirmative reply has been received yet. Linux Yama ptrace_scope=1 denied gdb attach to the existing GUI; perf is not installed. Timing environment was not enabled in the user's process.

Observed clicking saved 'Why Rust Has Richer LSP Hover Documentation' in the real debug GUI: old/blank pane still visible in computer.batch's ~528ms screenshot; later transcript appeared. SessionID ses_e9e98abbeffeDKU50a3FHhhm4V. Direct debug-backend timings: scoped SSE headers 234-377ms, history32 about16-28ms/79625bytes, metadata305-342ms. Correct SSE field is camelCase sessionId, NOT sessionID or session_id (unknown fields yield unscoped fast stream and misleading timings).

Added ignored opt-in native test pane/hydration_tests.rs::conversation_open_live_backend_timings. Run with NEOISM_SERVER=invalid://agent-unit-tests NEOISM_AGENT_BENCH_SERVER=<existing-backend> NEOISM_AGENT_BENCH_SESSION=<saved-id> cargo test -p neoism --bin neoism conversation_open_live_backend_timings -- --ignored --nocapture --test-threads=1. It invokes actual native switch/subscription/history/ingest without rendering or sending prompts. Before backend fixes: cold354073us, warm585us/209us,33blocks, longest ingestion drain403us. Constructor in this test is Default, not full context factory; cfg(test) omits normal prompt-history/config reads. Do not conflate with GUI click-to-present.

## Proven bugs fixed

v2_routes.rs::v2_events subscribed before snapshots but awaited session_family_ids before SSE headers. That helper used list_sessions() and full catalogue SessionInfo+hosting hydration. Since native root history correctly waits for subscribed headers, this scan was directly on the text critical path. Added SessionStore::session_descendant_ids in state.rs: canonical parent index, IDs only, 16-way UNION ALL equality frontiers, root retained even absent, visited IDs terminate cycles. Keeps parent-descendant semantics (NOT catalogue execution-root hint semantics). v2_routes.rs helper delegates to it, retaining root-only fallback. Root resolution, subscription ordering, replay/live/dynamic descendant and tenant/hosting admission are unchanged.

Critical correction to previous metadata optimization: Turso does NOT use idx_sessions_catalog_parent for json_extract(parentId) IN (...). Existing catalog_family_members still scanned despite having an index. Changed parent frontiers to 16-way UNION ALL equality seeks. Kept ancestor primary-key id IN because EXPLAIN proves actual id=? search for1,2,128 IDs. Larger128-way compound SQL overflowed Turso stack in agent tests; keep16. Metadata hint/ancestor/cycle semantics unchanged. New EXPLAIN tests use full production projected columns and assert per-branch index SEARCH/no table SCAN, rather than merely index existence.

Backend tests state/session_descendant_tests.rs cover fixed-point closure equivalence, missing roots, orphans, cycles, hints ignored for SSE, wide multi-batch frontiers, missing sidecars, unrelated invalid SessionInfo; scoped SSE headers plus child replay/live delivery survive unrelated invalid row. catalog_family_tests.rs now verifies actual parent/seed/ancestor plans.

## Frontend diagnostic changes

Added pane/render_state.rs::record_conversation_open_started and calls in screen/bridges/workspace/sidebar.rs. First-layout clock now includes the sidebar activation/context construction, not just switch_session internals. It does not manufacture a timer for already-painted panes. Native test conversation_open_sidebar_clock_includes_pane_construction covers this.

Existing NEOISM_AGENT_UI_PERF_LOG=1 or NEOISM_AGENT_PERF=1 enables new stage lines: 'agent conversation pane construction', 'agent conversation sidebar activation', 'agent conversation subscribed'; fetch log also includes request_us separately from full fetch/mapping elapsed_us. Existing 'agent conversation first transcript layout' now spans construction for sidebar opens. Use filtered RUST_LOG=warn,neoism::agent_ui_perf=info (unfiltered neoism=info spams per-frame server routing and loses useful output). Native background_task_result truncation can hide late stderr timings; operational tee to /tmp logs plus native grep/read worked. /tmp/neoism-chat-bench.log holds old native timing run; /tmp/neoism-chat-debug-build.log latest build; /tmp/neoism-chat-regressions.log native suite; /tmp/neoism-chat-backend-regressions.log backend suite.

## Final verification for this partial increment

cargo build -p neoism --bin neoism -p neoism-agent-server passed twice, including final confirmation after edits. No release build. Native targeted suite from previous memory plus new clock test:98passed,0failed,1ignored (the opt-in network benchmark). Separately ran that ignored network benchmark successfully against existing debug backend (before server updated). Backend consolidated cargo test -p neoism-agent-server --lib -- v2_ catalog_family_tests session_descendant_tests --test-threads=1:25passed. git diff --check passed. Extensive unrelated dirty changes remain and were preserved. Do not claim a fresh production check or updated GUI speed measurement: build/tests passed, runtime before/after remains unfinished.

## Resume

Await user's permission for one controlled restart of their existing workspace3 debug, not extra windows. If approved, preserve its current workspace and use NEOISM_AGENT_UI_PERF_LOG=1 RUST_LOG=warn,neoism::agent_ui_perf=info, stream output to inspectable /tmp log, let debug's usual isolated embedded backend own its existing dev data (do NOT override to production4096; that changes dataset). Reidentify actual agent port and existing saved session after restart. Measure indexed SSE headers, native cold pipeline, and logged sidebar-to-first-layout, then visually verify transcript. If multi-second delay remains, use stage timings to find it. Relevant remaining candidates: full context construction vs cfg(test) Default, cold text layout, per-frame chrome work; do not declare full solution solely from SQL/index tests.
