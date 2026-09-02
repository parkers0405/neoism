---
name: "Agent run spacing and lower tab ghost drag"
description: "Styled run spacing hardened and lower tabs cannot ghost-drag"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-08-30"
---

Follow-up fixes after v0.7.69: Agent Markdown run overlap persisted because the thread-local measure/caret cache key omitted DrawOpts.font_id and Sugarloaf scale factor, so widths could be reused across faces/windows/scales; key now includes both. Whitespace-only shaping fallback also used 0.35em, narrower than Neoism's monospace cell; it now uses current face measured `M` cell (with 0.5em floor), preventing inline-code/color runs crowding preceding punctuation. Lower Chrome/buffer tabs had the same lost-release latch as Island tabs: release handler was after many consumers and cursor motion ignored physical LMB state. Added non-committing BufferTabs::cancel_drag, Screen cancellation of exact source+previews, first-refusal lower-tab release after Island, cross-window source resolution, cancellation on fresh LMB press, physical LMB-up motion, and focus loss. Tests/checks pass.
