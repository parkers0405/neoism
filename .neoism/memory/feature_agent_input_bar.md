---
name: feature-agent-input-bar
description: "New-look agent input bar — outer shell + input box + dropdown chip row (click → / picker), square send button with running-loader glyph"
metadata: 
  node_type: memory
  type: project
  originSessionId: f68e5b0c-e85e-4e1d-a759-93fe8315bea1
---

Agent GUI input bar restyled (2026-07-06, user screenshot reference) in shared `panels/agent_pane/view/user_input.rs::render_input` — one implementation serves chat mode, the pre-chat home landing, desktop, and web.

Structure (v3, FLOATING ISLAND): the input box's own border is the island's top/sides; a "skirt" (border + hollow bg fill, same width, drawn from the box midline down, hidden behind the opaque box) shows only BELOW the box and wraps the dropdown chip row — the user rejected the earlier full outer shell because its top/side band read as "not floating". Two stacked black-alpha rounded rects give the shadow. `chat_input_rect`: side inset +8, bottom_pad 14, width cap 960 centered. Chips are `label ˅` (agent=accent, model=blue, thinking=magenta, NO icon — gear rejected); each registers `register_status_chip_rect(index,…)`; click opens the matching "/" picker via `open_status_chip_picker` (0=agent,1=model,2=thinking) — wired in desktop `bridges/agent.rs` and wasm `agent_pointer_down`. Send = white rounded-square bottom-right: idle = ↑ arrow; streaming = dark stop-square + pastel orbit loader (render_policy `loader_*`, side-panel spinner cadence; redraw driven by `animation_reason()`'s `is_streaming`). NO plus sign — explicitly rejected.

Heights: `HOME_INPUT_MIN_H=106`, `CHAT_INPUT_MIN_H=98`, `base_h` 84/76 in `layout.rs::input_height_for_width`, `CHIPS_BAND_H=26`. `render_input` takes `now_seconds`. Home landing: wordmark `h*0.12` clamp 34..84 anchored 44px ABOVE the input card (not band-floated — that left a huge dead gap), input at `y + h*0.46`. Agent picker filters `mode == "subagent"` (explore/general are Task-tool targets, not top-level agents) in desktop `api.rs::fetch_agent_options` AND wasm `agent_options_from_catalog` — shows session default + build + plan + user-config primaries.

**v4 refinements:** send glyph idle = ↑ arrow, busy = dark stop-square + pastel orbit (frozen-spinner root cause was the f32 epoch clock — see [[bug-f32-epoch-animation-clock]]); island shadow REMOVED (read as a smeared bg band on near-black themes); picker cards (/model etc.) occlude chrome text via `Renderer.agent_picker_occlusion` (set each frame in `render_neoism_agent_panels` from `picker_card_rect()`, folded into `active_text_occlusion_rects`) + `buffer_tabs::render_with_icons` now takes `occlusion_rects` and uses `draw_text_with_occlusion` for labels/close/+, skipping PNG agent icons that intersect a modal.

**How to apply:** any new clickable affordance in this bar follows the usage-chip pattern: trait method on `AgentUserInputPane` (+ macro + direct impl), rect field on BOTH pane copies (desktop `neoism/agent/pane.rs`, shared `state.rs`), hit branch in desktop `bridges/agent.rs` AND wasm `agent_pointer_down`. Up/Down in the draft walk renderer-registered soft-wrap byte ranges (`set_input_wrap_ranges` / `move_up_with_history_visual`) — history only from first/last visual row.

2026-07-16 (uncompiled, tomorrow's verify batch): usage %-chip moved from left-of-send-button to FAR LEFT of the same line (user_input.rs usage_x = box_x + 16*s; guard flipped to no-collide-with-send). Hit rect follows automatically (register_usage_chip_rect uses same vars).
