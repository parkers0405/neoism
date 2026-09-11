---
name: "TS GUI bottom activity fixed"
description: "Fixed bottom activity word and Background-vs-Crafting in standalone TS GUI; dock outside transcript/input scrollers, scoped provider segments, 483 tests"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-10"
updated: "2026-09-10"
---

Standalone TS GUI activity fix (source-only; no browser/restart): App.tsx owns one NativeActivity as the first child of the existing absolute bottom composer-dock, BEFORE composer-anchor and outside composer-content's capped scroller. Timeline showActivity=false in App prevents duplicate; legacy standalone Timeline default retained. Existing dock ResizeObserver includes status height in transcript clearance, including read-only child dock (no input/footer). No scroll positioning or new dock animation. ActivityPopover reposition listens only to window resize, not capture-scroll. Square footer scanner/hints remain separate.
Status: useChat returns activityBusy=raw state.busy before aggregate runtimeWorking (which includes unfinished execution/children/jobs). nativeActivityMotion checks unfinished provider activeSegments; consumes real server execution.sessionActivities per viewed session via local structural type (SDK generated schema currently omits field). Own provider stream > outstanding viewed child (Tinkering) > children (Sub-agents working) > jobs (Background) > idle. Empty activeSegments plus unfinished execution is NOT Crafting. Latest assistant must belong to viewed session and be incomplete; ended reasoning doesn't imply Pondering. NativeActivity idle+authoritative job counts promotes Background and retains native canvas dots/animation. Queues remain count-only when idle. Canonical sources read: shared state/streaming.rs raw_streaming_status, state.rs label(), status_policy.rs, view/user_input.rs. Server v2_session_runtime returns family aggregate + per-session provider map.
Verification: GUI tsc+Vite production build pass; 483 tests /49 suites pass, including fixed dock outside both scrollers, no duplicate, no scroll animation/reposition, background-only unfinished execution, provider+jobs, reasoning, completion, child/root scope, existing real job Stop & queue popovers. No browser/Playwright/cargo release/restart. Modified only targeted GUI activity/App/hook/test/CSS files. User emphatic: word must stay ABOVE input at bottom, never follow message scroll or pin to top viewport.
