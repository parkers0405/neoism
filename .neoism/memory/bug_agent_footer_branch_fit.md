---
name: "Agent footer Git fit uses actual space"
description: "Hide whole Git group only at measured hint boundary; removed premature 58%-width cutoff; 18 composer tests pass"
type: "bug"
scope: "project"
origin: "User correction to early Git hiding"
created: "2026-10-08"
updated: "2026-10-08"
---

Agent composer footer checkout context is rendered by shared/src/panels/agent_pane/view/user_input.rs, used by native and shared hosts. Right-side 'tab agents' and '/ commands' hints are measured before checkout painting. Git branch icon and full branch text must be drawn together only if their measured group fits after the directory and before the hints with existing 12 scaled-pixel separation.

Initial overflow fix incorrectly retained the old checkout clip `footer_width * 0.58`, causing Git to disappear even when real footer space remained. User explicitly rejected this early cutoff. Corrected: checkout_context_width derives available width from the measured hint left edge minus padding and footer origin, clamped to footer width. Both checkout clipping and branch fit use that actual space, never an arbitrary percentage. When hints are disabled, use footer right edge. Directory stays visible (clipped to available space); no partial branch/icon and no overlap with hints.

Verified cargo test -p neoism-ui --lib composer_visual_policy_tests: all 18 passed, including new regression for a branch extending past the obsolete 58% cutoff but fitting before hints, exact-fit/overflow, hidden hints, tight layout and 0.5/1/2/3 scale. Actual native cargo check -p neoism --bin neoism passed. No live GUI validation.
