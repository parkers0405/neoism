---
name: "Chrome overlays shifted by workspace Island — FIXED"
description: "Chrome palette and modal overlays no longer shift down when additional workspaces make the Island visible; anchor excludes Island entirely"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-02"
updated: "2026-09-02"
---

## Fix

Desktop overlay anchors in `neoism-frontend/desktop/src/host/run.rs` must never derive from `chrome_top`, because `chrome_top` includes the variable workspace Island height. Even the earlier `chrome_top(1)` workaround fails when `hide_if_single` is disabled.

Compute `overlay_top` from fixed window chrome only: `top_bar_strip_height() + buffer_tabs.height() + 8 * chrome_scale`, then apply it to command palette, Finder, search, and UniversalModal.

Verified with `cargo check -p neoism` (passes; pre-existing warnings only).
