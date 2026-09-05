---
name: "Shared BufferTabs long title + hover pill — FIXED"
description: "Single geometry budget and stable editor-tab hover fix for shared BufferTabs"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-03"
updated: "2026-08-03"
---

# Shared BufferTabs long-title / hover regression — fixed

Root cause: variable-width slot sizing measured title/icon at the base scaled font, but hover then scaled font/icon and recomputed the title clip inside the unchanged slot. Short active labels such as `Neoism` crossed the clip budget on hover and became `Neoi…`. Width and paint also independently derived icon/close reservations, and Panel-path hit testing inferred viewport width from overflowing content.

Fix in `neoism-frontend/shared/src/panels/buffer_tabs{.rs,/impl_core.rs,/impl_render.rs,/tests.rs}` plus integration tests:
- `TabVisualGeometry` is the single sizing/clip inverse: actual measured icon width, title budget, close half+gap exactly once, padding/overhang, 72px min and 220px cap, scale only on unscaled constants.
- Hover no longer scales tab/text/icon. Active/hover surfaces occupy the exact rectangular slot with 3px top corners, active top hairline, and integrated strip fills.
- Close reservation is stable; X paints only active/hover (dirty dot remains), so no width shift.
- Long capped closeable tabs expose 161.5px title budget at scale 1; ellipsis is emitted only if it fits.
- Shared-canvas tooltip shows the full title for a hovered truncated tab (not DOM-only).
- Actual strip viewport is cached separately from content width; variable-width overflow scroll/reveal remains, and `set_tabs` schedules active reveal.
- Existing variable-width drag/reorder/drop and direct touch scrolling retained.

Checks: focused shared unit tests 74/74 and integration tests 17/17 pass; `cargo check -p neoism-ui`; `cargo check -p neoism-ui --target wasm32-unknown-unknown --features sugarloaf/web`; `cargo check -p neoism --lib`; diff/targeted format checks pass. Full wasm package check is currently blocked by unrelated concurrent `status_line.rs` call to missing `Chrome::markdown_pane()` (E0599/E0282).
