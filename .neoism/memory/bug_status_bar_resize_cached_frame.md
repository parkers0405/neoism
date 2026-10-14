---
name: "Status bar resize cached-frame defect"
description: "Native resize left cached surface frame stale; resolved before grids, DPI unified, explicit viewport repair; 8 native binary tests passed"
type: "bug"
scope: "project"
origin: "Cross-platform native status bar resize investigation"
created: "2026-10-07"
updated: "2026-10-07"
---

User reported bottom status bar sometimes tucked up into the native window during resize on Windows/macOS/Linux. Found concrete shared desktop defect: Screen::resize updated Sugarloaf and grids but left Renderer::surface_layout cached; host/run.rs anchors status to surface_layout.content. Pre-paint chrome signature omitted physical viewport dimensions/device scale, and indirect bottom-inset recovery could miss shrinking or width-only changes. This is a credible cause; the exact live visual symptom was not reproduced.

Fixed in neoism-frontend/desktop/src/screen/chrome_geom.rs and screen/mod.rs: extracted update_surface_chrome_margins shared by resize and reapply; resolve current surfaces/margins before the existing single grid resize; snapshot valid chrome signature afterward. set_scale refreshes all pane cell dimensions and uses resize rather than duplicate/out-of-order layout passes. Signature includes physical window width/height and DPI bits. Pre-paint explicitly checks cached viewport against the surface solver's snapped logical viewport (not content, which may legitimately be inset by plugin docking). Constructor resolves actual viewport before first paint, retaining valid first-frame no-reflow behavior. Preserves configured bottom-docked surfaces; no paint-only hardcoded status-height workaround.

Verification: cargo check -p neoism --bin neoism passed; cargo test -p neoism --bin neoism launch_layout_tests passed all 8 tests (3 new tests cover grow/shrink/width-only/DPI signature invalidation, first-frame stale viewport, fractional chrome/DPI snapping and legitimate bottom docking). Targeted rustfmt and git diff --check passed. Important: desktop Screen modules/tests are in the BIN, not --lib; initial --lib filter ran zero geometry tests. No release build or live Windows/macOS/Linux resize validation.
