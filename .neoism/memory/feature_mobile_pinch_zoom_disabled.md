---
name: "Mobile/web pinch zoom disabled"
description: "Shared+web policy suppresses divergent pinch while preserving two-finger pan and keyboard viewport behavior"
type: "feature"
scope: "project"
origin: "implementation"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented mobile/web pinch-zoom disablement. Shared `touch_policy.rs` still distinguishes same-direction two-finger pan (TwoFingerScroll) from divergent pinch, but committed pinch now always emits SuppressNativeGesture in TerminalBody, EditorArea, and ChromePanel; it never emits ChangeFontSize. Web `touchPolicy.ts` also normalizes legacy/stale wasm `change-font-size` actions to suppression, and TerminalPanel defensively keeps font scale unchanged for touch actions. Browser page zoom is disabled via viewport `maximum-scale=1,user-scalable=no`; terminal canvas, markdown layer, mobile capture/key surfaces use `touch-action: pan-x pan-y` (pan allowed, pinch excluded). visualViewport remains normalized only for keyboard inset observation; canvas CSS geometry/DPR contract does not consume visualViewport scale. Tests cover all three zones, stale wasm scale invariance, two-finger pan, viewport meta, and CSS/DPR geometry stability. Checks: 107 web tests pass; TS typecheck passes; `cargo test -p neoism-ui touch_policy --lib` 29 pass; debug wasm `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web` passes. Do not rebuild wasm with `npm run build:wasm` for checking because wasm-pack defaults to release; use debug cargo check. Concurrent modal/file-picker edits were preserved.
