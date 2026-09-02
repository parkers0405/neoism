---
name: "Agent wheel spring vs trackpad inertia"
description: "Agent mouse wheels use a three-row code-style fixed-target spring while touchpad PixelDelta behavior remains unchanged; desktop/shared implementations mirror each other."
type: "feature"
scope: "project"
origin: "Implemented after comparing agent/Markdown scrolling with code-pane scrolling"
created: "2026-08-11"
updated: "2026-08-11"
---

Agent timeline input has two intentionally distinct paths. Precision trackpads (`PixelDelta`) retain direct 1:1 response plus the existing exponential kinetic glide. External mouse wheels (`LineDelta`) map one notch to three physical text rows and accumulate a fixed `timeline_wheel_target_px`; rendering advances toward it with the same critically damped spring constants as code panes (`OMEGA=16`, fixed substeps at 1/240s). Repeated notches extend the deterministic target rather than compounding free-running velocity. Shared and desktop agent pane implementations must remain mirrored. Streaming relayout shifts both current scroll and the active wheel target so the reader anchor remains stable.
