---
name: bug-shader-terminal-stutter
description: "Animated shader overlay stutters only on terminal pane — shader is not a registered animation owner in redraw_reason(), idle terminal schedules no frames"
metadata: 
  node_type: memory
  type: project
  originSessionId: 8c8ab78f-b2bc-4a51-9d08-a3e1f6c67725
---

The `:shaders` overlay animates fluidly everywhere except an idle terminal pane. Root cause (confirmed): shader time is wall-clock sampled per drawn frame (`sugarloaf/src/components/shader_overlay.rs:77-104`, vulkan `renderer/vulkan.rs:1180-1189`), and nothing in frame scheduling knows the overlay exists. The scheduler only keeps frames flowing while `redraw_reason()` (`neoism-frontend/desktop/src/host/state.rs:145-235` — the single aggregator of all continuous-animation owners) returns Some, plus dirty flags; present is additionally gated by `should_present_frame` (`shared/src/render_policy.rs:1214`). Every other surface always has an owner alive (agent streaming, scroll springs, splash pulse — splash reports always-animating, which is why the EMPTY terminal is fluid); an idle prompt has zero owners → frames only on PTY damage/cursor blink → wall-clock iTime jumps per sparse frame = stutter.

FIXED (2026-07-03): registered shader overlay as an animation owner — `shader_overlay_active` flag on Renderer, set in `Screen::apply_shader_overlay` (`screen/chrome_geom.rs`), returned last from `redraw_reason()`. That wires both the event-loop scheduler (`app/mod.rs:732`) and the present gate automatically; frames become vsync-paced via `wait_until()` like editor_scroll/agent already are.

Note: `[renderer] shader-overlays` config only populates the picker; the modal picker is the sole enable path. librashader filters path is frame_count-driven (would slow down, not jump) but starves the same way.
