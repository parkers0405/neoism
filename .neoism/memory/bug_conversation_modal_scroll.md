---
name: "Conversation modal mechanical scrolling fixed"
description: "FIXED modal row quantization: shared pixel Scroll for list/highlight, clipped hits, direct touch, no fixed-time snap or glide tail"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-18"
updated: "2026-10-18"
---

# Conversation/project modal smooth scrolling

Fixed row-stepped scrolling in shared `neoism-frontend/shared/src/widgets/conversation_navigator.rs`. Old state used `offset: usize` plus a wheel accumulator, truncated deltas into 46-pixel rows, instantly revealed keyboard selection, and passed zero list/cursor residuals to an already capable inline-picker renderer. Neither host lacked redraw ownership: active navigator already requests continuous desktop frames and browser RAF.

The navigator now uses existing `widgets::scroll::Scroll` twice (list and selection highlight), wrapped by `NavigatorScroll` with `web_time::Instant`. Target-following, positive-down, unscaled logical pixels; 0.12 spring tuning, dt capped at 50 ms. Rendering derives first row=floor(current/46) and residual=first*46-current, and cursor residual=current_cursor-selected*46. Wheel deltas retain sub-row pixels, arrows/hover animate the highlight, and keyboard reveal moves the list target. Do NOT enforce a hard 120 ms deadline: the critical spring still has a visible remainder at that time, causing a final snap. Let it settle and snap only within 0.25 logical pixels, with no separate velocity/inertia loop.

Hit rows use the identical snapped residual as painting, include a partial bottom overscan row, and intersect with the body clip so header/footer cannot activate clipped rows. Refresh preserves current and target row identities, fractional displacement and in-flight cursor motion. Query changes reset/snap cleanly while revealing the current project; actual capacity changes reveal selection, but unchanged frames must NOT re-reveal or wheel scrolling gets pulled back to an offscreen selection.

`scroll(delta)` remains smooth-wheel API. `scroll_immediate(delta)` applies finger movement from the painted position and cancels both animation tails. Direct touch callsites were changed in `shared/src/chrome/events.rs::touch_scroll_at` and `wasm/src/rendered/palettes_finder.rs::modal_touch_scroll`; normal wheel routing stays unchanged. No JS animation or independent desktop spring was added. Preserve immediate touch/no lingering glide per `feedback_scroll_feel.md`.

Verification: 21 navigator tests and 8 chrome navigator-routing tests passed; WASM browser-target check passed. Includes sub-row movement, cursor motion within visible rows, clipped hit tests at multiple scales, touch cancellation, refresh/resize/query reset, and no visible deadline snap.
