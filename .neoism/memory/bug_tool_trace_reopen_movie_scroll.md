---
name: "Tool arrival full-page catch-up scroll"
description: "One tool reopened hidden last-turn trace and charged its whole height as live follow; 48px row produced 2280px catch-up. Mirrored disclosure provenance fix; tests/checks passed."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Tool arrival animated an entire old turn

User reported intermittent full-page downward animation after leaving/returning to a chat, then clarified it also occurs without navigation when one tool call arrives. Confirmed cause, not a pagination assumption: `retain_current_turn_trace()` opens from the last user prompt, revealing historical reasoning/tools hidden by the settled projection. Upserts/reasoning deltas already set `timeline_live_growth = true`, so metrics charged the whole disclosure as newly streamed content.

Deterministic headless regression uses production row visibility/grouping/packing and height estimates, not GPU exact measurement. One 48px read tool reveals 60 reasoning lines and 20 historical two-member read groups: rows 2 -> 24, content height 715 -> 2995, automatic lag 2280px. Test failed before fix and passes afterward with offset zero. Already-open group creation is a passing control.

Mirrored fix: shared state/timeline.rs and desktop pane/ingest.rs mark transient `timeline_trace_reveal_pending` when a closed trace with existing old trace rows opens. Metrics consumes it and clears automatic live-growth provenance before the first disclosure measurement. Marker survives multiple upserts before paint and resets on session navigation. Subsequent new output retains smooth follow, existing live lag can settle, and manual reader positioning/wheel/trackpad physics are unchanged. Do not apply a blanket pagination/visibility/height rewrite for this bug. Tool fog/motion, footer rendering, spring constants and expansion ownership remain unchanged.

Verification: shared same-chat layout controls 2 passed; shared follow-scroll 23 passed; desktop follow-scroll 20 passed. `cargo check -p neoism -p neoism-ui`, wasm32 check with web feature, and `git diff --check` passed. Initial combined job timed out after passing tests during compile checks due concurrent Cargo lock waits; remaining native/wasm checks were rerun successfully. Related: [[bug_conversation_load_animated_follow]], [[feature_smooth_stream_follow]].
