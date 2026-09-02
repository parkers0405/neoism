---
name: feature-cursor-style
description: User cursor color override + rainbow preset; propagates to multiplayer peers via presence rainbow flag
metadata: 
  node_type: memory
  type: project
  originSessionId: 08b48318-c083-4091-b4a1-3f2ed0c7cda3
---

Cursor styling (built 2026-06-12): `[neoism] cursor-color = "#RRGGBB"` overrides theme accent (re-applied inside `set_ide_theme` so it survives theme switches); `[neoism] cursor-style = "rainbow"` animates hues and ignores color; `[neoism] blinking-cursor = true` is an alias folded into `cursor.blinking` by `Config::apply_neoism_aliases` (called in both `load()` and `try_load()`).

Key wiring:
- Shared logic in `neoism-frontend/shared/src/cursor_style.rs` — `CursorStyle`, `parse_hex_color`, `rainbow_color_f32(rainbow_now_seconds())`. The clock is a `web_time::Instant` process epoch (NOT unix-epoch f32 — that quantizes to ~128s and freezes animation; the agent pane's `SystemTime` now_seconds has this latent bug).
- Desktop: `Renderer::live_cursor_color()` / `cursor_is_animated()` in host/state.rs; rainbow wins over OSC 12 in render/mod.rs grid path; `needs_redraw` returns true while local style animated OR `remote_rainbow_active` (refreshed each frame from `RemotePresenceStore::any_rainbow()`).
- Multiplayer: `rainbow: bool` (serde-defaulted) on `CrdtPeerPresence` + `PeerPresence` + cue structs (`EditorRemoteCaret`, `MarkdownRemoteCursor`, `MarkdownRosterEntry`); publishers send `named_colors.cursor` + rainbow flag; painters animate peers locally on the shared clock — never stream colors over heartbeats.
- Web: chrome `set_cursor_style_config` via wasm `set_cursor_style`; local picking reads localStorage `neoism.cursor-color` / `neoism.cursor-style` in TerminalPanel's `applyPresenceThemeColor`; `animations_active()` includes `rainbow_cursor_active()`.
- Future presets slot into `CursorStyle` enum + the same presence flag pattern. Related: [[project-markdown-editor]].

Blink persistence gotchas (fixed 2026-06-12, watch for regressions):
- NEVER seed a new context's blink from `renderable_content.has_blinking_enabled` — it's a render-time mirror, false before the first frame; workspace restore on startup created panes with blink permanently off. Use `ContextManagerConfig::cursor_blinking` (config-sourced, refreshed on reload in chrome_geom).
- DECSCUSR 0 (cursor reset, nvim sends on exit) must restore `Crosswords::default_blinking_cursor`, not the derived `false` — mirror of `default_cursor_shape`. Explicit steady requests (CSI 2/4/6 q, ?12l) stay honored.
- Cadence: default interval 530ms (was 800); typing hold = one interval with phase anchored to last keystroke (was flat 1s + fresh phase ≈ 1.8s frozen).
