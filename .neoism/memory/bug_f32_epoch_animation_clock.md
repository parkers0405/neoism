---
name: bug-f32-epoch-animation-clock
description: Raw unix epoch as_secs_f32() freezes animations — f32 at ~1.7e9 resolves ~128s steps; always wrap with animation_phase_from_unix_secs
metadata: 
  node_type: memory
  type: project
  originSessionId: f68e5b0c-e85e-4e1d-a759-93fe8315bea1
---

The agent pane's send-button loader "drew but never animated": `bridges/agent.rs` fed `SystemTime::now()...as_secs_f32()` as the view's `now_seconds`. An f32 mantissa (24 bits) at ~1.7e9 seconds only resolves ~128–256 SECOND steps, so the clock was effectively constant between frames — every clock-driven animation keyed on it (loader orbit, shimmer) froze while redraws ran fine.

**Why:** f32 precision, not a redraw problem. The repo already has the correct helper: `neoism_ui::render_policy::animation_phase_from_unix_secs(secs, subsec_nanos)` wraps at 10_000s so the mantissa keeps sub-millisecond resolution (same convention as `host/composer.rs`'s `% 10_000` phase).

**How to apply:** any animation clock derived from wall time must wrap before the f32 cast — grep for `as_secs_f32()` on `UNIX_EPOCH` durations when an animation "draws but doesn't move". Phases must only be used modularly (sin/frame-index); never compare a wrapped phase against a stored absolute time.
