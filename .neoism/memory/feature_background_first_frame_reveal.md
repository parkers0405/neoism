---
name: "Picture-first window reveal, no startup flash"
description: "No naked first window: wait for configured image then reveal once. Wayland/X11 actual launches validated; fixes hidden-X11 explicit redraw delivery."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Background in the very first visible frame

User explicitly prefers a slower first uncached launch over any visible plain/naked-background frame. Cached launches must also have the picture from their first frame. This supersedes the earlier async paint-first/show-picture-later policy. Keep worker/cache, global blur default0 and user's explicit blur3 unchanged.

## Reveal lifecycle

RouteWindow (desktop router/window.rs) owns a separate one-shot initial_reveal_pending flag; existing has_rendered_frame is only cadence bookkeeping and is not proof of presentation. Native X11/macOS windows start hidden. Windows stays visible-but-cloaked so hidden HWND redraw suppression does not deadlock; uncloak moved from Screen construction to successful-first-frame reveal. Wayland set_visible is a no-op: withhold the first content-buffer presentation while image pending, which keeps the surface unmapped.

Common app/window_event/dnd.rs redraw path polls the background worker BEFORE begin_render and before any pre_present_notify, for both Welcome and terminal routes. If initial_reveal_pending && pending_background_image().is_some(), return without presentation or frame callback. Worker success installs pixels, failure drains/clears pending and reports warning, no-image/clear also allow normal/error first frame. Do not wait on !background_image_ready(): that stays false forever for no image/failure.

Record before_present_called per invocation. Reveal only when it was called and Sugarloaf.last_frame_presented() is true, then latch initial_reveal_pending false. Constructor last_frame_presented starts true and Screen can early-return, so checking that flag alone is unsafe. Render-completed trace now reports actual invocation callback AND present flag; trace initial window revealed after set_visible/uncloak.

Renderer::render_metal now returns bool, false on drawable acquisition/type/buffer-cap failure and true only after present+commit; Sugarloaf propagates it rather than always claiming success. Mac/Windows runtime not available on Linux; Linux Wayland/X11 are physically validated.

## Wake/retry policy

Initial worker Render wakes bypass unfocused/occluded suppression in app/user_event_dispatch.rs and directly request native redraw, even while pending is still Some (only redraw handler consumes completion). app/mod.rs event-loop redraw scheduling sleeps while image pending, retries unresolved first presentation at normal cadence, and returns to ordinary policy after reveal. Never call Wayland pre_present_notify on the pending skip path: that would arm a callback without a content commit.

## X11 hidden redraw fix

Actual X11 smoke initially hung: cache prepared but zero RedrawRequested events. Created-hidden X11 windows have no vsync source, yet explicit redraw_pending was previously drained only by mapped/visible vsync. Native x11/window.rs adds initial_hidden_redraw initialized !visible. event_processor.rs permanently clears it at first MapNotify. x11/mod.rs snapshots and consumes explicit redraw requests for those initial-hidden windows after user events, forwarding to the existing coalesced redraw queue. Only explicit work is drained; no timer/spin, callbacks cannot recursively reenter the same drain. Subsequent hidden/occluded windows preserve normal dirty/vsync policy.

Also retain the first has_pending readiness result across the post-poll check: the first call consumes redraw_flag, so checking again alone can discard a redraw-only wake as spurious. Tests cover explicit/coalesced/once-only worker redraws and preservation after MapNotify.

## Verification

Final latest debug build actually launched with isolated config/workspace/helper/GUI IPC. Wayland coldblur3 first render3325.67ms, wallpaper3325.67ms, reveal3325.69ms; warm first/wallpaper574.90ms, reveal574.95ms. X11 warm first/wallpaper1047.33ms, reveal1047.45ms. All successful-image cases first_frame_background_ready=true, cache status verified, and own PID confirmed mapped through hyprctl clients. Missing image fallback and no-image both reveal normally (~0.54s/~0.53s), not invisible forever. /tmp/neoism-background-bench/measure.py now waits for reveal and checks compositor mapping; source image/cache/opacity behavior unchanged.

Tests: cargo test -p neoism --bin neoism initial_reveal_tests (2), cargo test -p neoism-window --features x11 --lib initial_hidden_redraw_tests (2), Sugarloaf background tests with neoism-window/x11 (21), wasm web cargo check and git diff --check passed. Static review found no concrete introduced bugs. All benchmark windows/helpers closed; live user instances untouched. Changes remain local, no commit/push.
