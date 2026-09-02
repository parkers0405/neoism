---
name: bug-web-agent-caret-no-animate
description: Agent composer caret invisible on web — AgentInput arm never called trail_cursor.animate() — FIXED 2026-08-19
metadata:
  type: project
---

On web the agent pane composer accepted focus and typing (text appeared) but NO
caret ever painted, before a chat and in one. Not focus, not the blink clock, not
occlusion — a geometry bug.

The composer does not draw its own caret; `render_input` only publishes a rect and
the shared `TrailCursor` overlay paints it. In `shared/src/chrome/draw.rs` the
`TrailCursorOverlayTarget::AgentInput` arm called `set_destination(...)` then
`draw_always(...)` but never `animate()`. `TrailCursor::draw_quad` builds its
triangles exclusively from the spring-animated `corners[i].x/.y`, and those are
written ONLY by `animate()` / `snap_to_destination()`. `Corner::new` starts at
(0,0), so the quad stayed zero-area = nothing rendered. Since `AgentInput` is the
only arm taken while the agent tab is active, nothing else advanced it either.

**Why:** every other target goes through `draw_block_trail_cursor_rect` /
`draw_content_trail_cursor_rect` (or the terminal arm), all of which animate;
`AgentInput` was the lone outlier. Desktop's own `render/overlays.rs` arm does
call `animate()`/`snap_to_destination()`, which is why desktop was fine.

**How to apply:** `set_destination` alone is never enough to make the trail cursor
visible — it only records a target. Missing `animate()` also means
`is_animating()` stays false, so the caret never registers as an animation owner
in `Chrome::animations_active()` and the host stops pumping frames for it.

Related: [[feature-cursor-style]], [[bug-shader-terminal-stutter]],
[[feature-agent-input-bar]]
