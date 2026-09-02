---
name: feedback-scroll-feel
description: "Scroll-feel rules — smooth animated wheel notches yes, lingering glide tail after the gesture ends no; tune inertia per input device"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: c6967401-4270-436e-9d09-bcdf40124a24
---

External mouse wheels should feel smooth (animate each notch — small immediate nudge + velocity — instead of a big instant jump), but NO long glide after the gesture ends, on either device. A low stop threshold (4 px/s) that let motion crawl on after fingers/wheel stopped was explicitly rejected for both trackpad and wheel.

**Why:** User rejected the July 2026 change that dropped the timeline stop threshold to 4 px/s for all devices ("added time acceleration after a trackpad scroll is not good, the extra scroll time on external too") while insisting the wheel smoothness itself be kept.

**How to apply:** Capture inertia tuning per gesture at injection time. Agent timeline settings (duplicated in desktop `neoism/agent/pane.rs` and shared `panels/agent_pane/state.rs`, `TIMELINE_*` consts): trackpad = 1:1 immediate, 7x velocity, tau 0.28s, stop 50 px/s; wheel = 0.2x immediate, 12x velocity, tau 0.12s, stop 30 px/s (~36px settling in ~0.27s per notch). Device split comes from `agent_timeline_wheel` in `editor/scroll_model.rs`: LineDelta = wheel (smooth=true, 24px/notch, clamp ±3), PixelDelta = trackpad. Apply the same principle if markdown/other panes get scroll-feel complaints.
