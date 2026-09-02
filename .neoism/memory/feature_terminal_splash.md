---
name: Terminal pane splash (SVG image overlay)
description: NEOISM splash where the wordmark is a rasterised SVG painted via sugarloaf image_overlay; cells reserve vertical space + tagline + keybinds; opencode-style ambient pulse + click ripple
type: project
originSessionId: 3baab196-5a07-445e-ab73-5a54250cf719
---
Two layers:

**Cell layer** — `frontends/rioterm/src/renderer/terminal_splash.rs::splash_bytes(cols, rows) -> Option<String>`. Emits ANSI bytes that:
- Vertically center the splash with `(rows - SPLASH_HEIGHT - 1) / 2` leading newlines
- Reserve `WORDMARK_RESERVE_ROWS` (8) blank rows for the GPU image
- Print the centered tagline + 3 centered keybind rows below
- The wordmark itself is *NOT* in the cell stream — those rows are blank by design

Injected from `Renderer::run` (not `create_context`) once the pane's `(cols, rows)` has been stable for `SPLASH_STABLE_FRAMES` (4) consecutive frames — defers until layout settles so centering math uses the live width.

**GPU layer** — `frontends/rioterm/src/renderer/splash_overlay.rs::SplashOverlay`. Mounted on `Renderer.splash_overlay`. Each frame the splash is visible:
- Registers the rasterised PNG (`assets/splash/neoism-wordmark.png`, baked from `assets/splash/neoism-wordmark.svg` via `rsvg-convert -w 1200`) with `sugarloaf.image_data` once via `register_wordmark`
- Computes the image rect from `pane_origin + cell_h * wordmark_row` (band top) + pane width centering, capped at 70 % of pane width
- Calls `clear_image_overlays` then `push_image_overlay` per frame so stale overlays don't accumulate
- Paints a sin-wave ambient white rect over the image (alpha 0 → 0.07, 4.6 s period)
- On `pop_ripple(x, y)` (called from `Screen::handle_splash_overlay_click` via `application.rs` MouseInput pressed-Left chain), spawns an expanding white annulus polygon with eased radius + double trailing ring; decays over 720 ms

**Visibility predicate** (mirrored in renderer + screen):
- pane is terminal (no editor, no markdown)
- `terminal.history_size() == 0` (true at startup AND after `clear` — `clear` emits CSI 3J which wipes scrollback)
- not in alt-screen (nvim/htop/less)
- `splash_injection` recorded on `Context`

`Context` carries: `pending_splash`, `splash_dim_stable_frames`, `splash_last_dim`, `splash_injection: Option<SplashInjection { wordmark_row, wordmark_col, wordmark_cells_w, wordmark_cells_h }>`.

**Why image not cells:**
First two attempts used hand-designed pixel-letter art in cell space. Looked thin, lost depth shading, and reflowed when the file tree opened (cells reflow on width change, but a centered overlay re-anchors against the live pane rect each frame).
The SVG (`assets/splash/neoism-wordmark.svg`) has 3 stacked layers at opacity 0.30/0.50/1.0 with diagonal offsets — depth/shadow is baked in. Source provided by user from `~/Downloads/1.svg`.

**How to apply:**
- Re-rasterising: `rsvg-convert -w 1200 frontends/rioterm/assets/splash/neoism-wordmark.svg -o frontends/rioterm/assets/splash/neoism-wordmark.png`
- Aspect ratio is hard-coded in `WORDMARK_ASPECT` (1.0 — source SVG is 1500×1500). If swapping in a non-square wordmark, update that constant.
- Re-injecting on `clear`: not yet wired. `clear` resets scrollback so the splash vanishes — to bring it back, watch for CSI 3J in the parser and re-feed `splash_bytes` (deferred).
- Sound on click: `cpal` is gated behind an optional `audio` feature in `frontends/rioterm/Cargo.toml`. Wire the assets in `splash_overlay.rs` to play one of `pulse-a/b/c.wav`-equivalents on `pop_ripple` to match opencode's feel.

**Reference — opencode fidget mechanics** (sst/opencode `packages/opencode/src/cli/cmd/tui/component/{logo,bg-pulse}.tsx`):
- ambient breathing via sin(t/period) ~4.6 s
- click spawns `Ring{x,y,at,force,kick}`; brightness = crest(|dist - head|) within WIDTH band
- hold-then-release: glyph at click point holds briefly then "rises" with glow trail
- background pulse: 3 concentric rings emanating from logo center, masked away from chrome regions
