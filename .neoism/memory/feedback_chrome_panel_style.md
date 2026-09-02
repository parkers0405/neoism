---
name: Chrome panels must mirror the file_tree visual system
description: When adding new chrome surfaces, copy the file_tree's frame, row, scroll, and theme treatment instead of inventing new styling.
type: feedback
originSessionId: 97126cba-f109-488f-9a2f-2b00efd4a19c
---
New chrome surfaces (right-side git diff panel, future side panels) must
look and feel like `renderer/file_tree.rs` exactly — the user rejected a
panel that invented its own chrome (alpha bg, custom shadow, cyan icon,
freehand row layout) as "looking like piss" because nothing matched.

**Why:** The user spent real effort tuning the file_tree's visual system
(frame radii, surface/bg layering, edge_row_radii, spring scroll,
strict theme tokens) and treats it as the canonical chrome look. New
panels that diverge feel foreign and pollute the IDE's identity.

**How to apply:**
- **Frame:** outer `theme.surface` quad with leading top corners
  rounded (`[radius, radius, 0, 0]`) wrapping an inner `theme.bg` quad
  inset by `frame_stroke`. Same constants as file_tree
  (`FRAME_RADIUS = 14`, `FRAME_STROKE = 2.25`).
- **Selection:** `theme.surface` row bg via `quad` with
  `edge_row_radii` so corners blend into the frame at the top/bottom of
  the viewport, plus a leading accent stripe (`theme.accent`) using
  `edge_left_row_radii`.
- **Scroll:** `CriticallyDampedSpring` with
  `SCROLL_ANIMATION_LENGTH = 0.30` for both wheel and any keyboard
  scroll, snapped to device pixels via `snap_to_device_px`.
- **Theme tokens only:** fg / dim / muted / surface / bg / border /
  accent / red / green / yellow / blue / magenta / cyan. No alpha tints
  on the panel bg itself; alpha is acceptable only for content
  overlays (e.g. green/red diff line tints).
- **No bleed-through requires chrome reflow, not z-order tricks.** The
  rio render pipeline draws grid → rects → UI text in three passes with
  alpha blending, no depth test. UI text from buffer_tabs, status_line,
  command_composer, etc. is appended to a single text-instance buffer
  and painted last regardless of submission order, so nothing the
  panel does in `text_mut()` keeps that chrome text out of its column.
  The fix is to *reflow* — extend `Screen::reapply_chrome_layout` to
  push `right_scaled = chrome_x_offset_right * scale` into every
  `grid.update_scaled_margin`, and clamp `Renderer::workspace_strip_bounds`
  + the `status_line.render` width by the same right inset. Trigger
  `reapply_chrome_layout` on every visibility/focus change. Exposing
  `active_rect()` for `active_text_occlusion_rects` is still useful for
  surfaces that already use occlusion-aware drawing (finder, palette,
  file_tree), but it's not sufficient on its own.
- **Visibility = snap, not slide.** Animated slide-ins fight chrome
  reflow — the editor reflows instantly on toggle while the panel is
  still mid-animation, leaving a visible empty stripe. Match
  file_tree: panel appears/disappears immediately. Spring-damped
  scrolling stays inside the panel; the panel itself never animates
  in/out.
- **Animation:** none for visibility. Spring scroll for content.
