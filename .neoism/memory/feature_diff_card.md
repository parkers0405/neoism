---
name: "Reusable diff_card UI element"
description: "ApplyPatch cards stay fixed size, do not move viewport, and scroll full diffs internally"
type: "feature"
scope: "project"
origin: "Corrected Agent ApplyPatch interaction contract"
created: "2026-07-14"
updated: "2026-07-14"
---

# ApplyPatch diff cards use fixed-height internal scrolling

- Correct UX contract: clicking an ApplyPatch file must never change the card/timeline height or move the viewport. It switches from preview rows to the full diff behind the same fixed preview-height viewport.
- Expanded cards compute `max_scroll = full_body_h - fixed_body_h` and register the existing nested diff scroll rectangle. Wheel input over the body scrolls internally; edge hits bubble back to timeline scroll.
- Measurement and rendering both use `fixed_diff_viewport_height(view.preview_visual_rows, s)` regardless of expanded state. Following cards and viewport position therefore remain stationary.
- Per-file keys are `message_id:section`. Their toggle path must not create `pending_timeline_anchor`/`timeline_view_anchor`, stop timeline inertia, invalidate timeline geometry, or start height animation. It only toggles expanded state; rendering updates naturally.
- Generic geometry-changing tool cards retain the old anchor/invalidation/animation behavior.
- Regressions: `expanded_diff_keeps_the_collapsed_viewport_height`, `diff_file_toggle_does_not_move_or_reanchor_the_timeline`, and existing `interaction_policy::tests::diff_scroll*`. Shared/desktop checks and debug build pass.
