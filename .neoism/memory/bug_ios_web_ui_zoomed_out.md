---
name: "iOS web UI zoomed out / visualViewport scale"
description: "iOS hidden editable auto-zoom + raw visualViewport height conflated pinch with keyboard; fixed with CSS-pixel viewport contract and normalized keyboard observation"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-09-04"
updated: "2026-09-04"
---

# iOS web UI scale / visualViewport fix

Root cause (2026-09-04): the invisible mobile contenteditable inherited the mobile 13px body font, triggering iOS focused-control auto zoom. MobileKeyboard also compared raw `visualViewport.height` with the layout viewport and ignored `visualViewport.scale`, so browser pinch/auto zoom was misclassified as a bottom keyboard inset. This mixed three distinct coordinate spaces: layout CSS px, visual viewport zoom, and render DPR.

Fix:
- `mobileEditingPolicy.ts`: `keyboardViewportObservation` removes pinch from visual extent (`height * scale`, with offsetTop already in layout CSS px); visual width/scale never become layout width/chrome scale. `mobileViewportLayout` pins width/full render/status dimensions in layout CSS px and only carves editableBottom by keyboard inset.
- `MobileKeyboard.ts`: feeds normalized visual height to shared Rust keyboard-inset policy/fallback.
- `TerminalPanel.ts`: root client width/full height remain canvas + Chrome layout truth; keyboard inset is separately replayed to WASM shared Chrome. DPR remains only backing/raster scale through existing sizeContract and texture-cap handling.
- `mobile.css`: hidden capture has 16px font to suppress iOS control auto zoom; text-size-adjust 100% prevents orientation autosizing without disabling pinch.
- Shared Chrome bottom_content_inset keeps status physically bottom while terminal/composer stop at keyboard top.
- Fixtures cover iPhone 390x844 @ DPR3 before/after 301px keyboard and visualViewport scale=2; assert width/chrome scale/render backing unchanged.

Checks: web tsc passes; 14 targeted mobile viewport + sizeContract tests pass; cargo check neoism-terminal-wasm wasm32 --features web and neoism-ui pass; git diff --check passes. Full npm test had 3 unrelated stale touch-policy expectations against concurrent Rust touch behavior.
