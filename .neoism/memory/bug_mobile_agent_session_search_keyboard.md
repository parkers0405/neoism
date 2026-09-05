---
name: "Mobile Agent session-search keyboard ownership — FIXED"
description: "Exact field hit intent, deferred clean-tap focus, scroll cancellation, routing and blur lifecycle for iOS Agent side-panel session search"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-08-01"
updated: "2026-08-01"
---

# Mobile Agent session-search keyboard ownership — FIXED

The Agent side-panel session search is now an exact mobile text-field owner.

- Shared renderer records the exact painted session-search rectangle each frame; WASM `mobile_text_field_at` returns `agent-session-search` only inside that rect while side-panel takeover is active.
- Do not fold side-panel takeover into generic `pointer_overlay_active`: doing so diverts taps away from `agent_pointer_down` and breaks row/button routing. Web instead uses a keyboard-only `mobileTextOverlayActive()` boundary.
- Search uses trusted deferred `touchend` focus. A clean first tap focuses the hidden capture; scroll promotion cancels without ever focusing; row/button misses cannot fall through to the Agent composer.
- WASM routes focused search Backspace/Escape/printable text before generic side-panel key policy. This changes the shared query, not hidden composer text.
- Web compares pre/post side-panel takeover and search-focus states after pointer/key handling, blurring capture when navigation closes takeover or leaves search.
- Keyboard viewport/inset redraw preserves committed focus through `reconcileCommittedTouchKeyboardFocus`.

Regressions: web tests `session search first tap resolves to one deferred overlay focus`, `session search scroll cancellation never focuses or falls through`, `session search typing route preserves every direct input commit`; shared test `session_search_typing_route_owns_only_the_painted_field`.
