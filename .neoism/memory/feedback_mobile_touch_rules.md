---
name: mobile-touch-rules
description: "Web touch invariants — touch pointers excluded from PointerEvent handlers, keyboard inset must survive every handleResize, agent clicks via agent_pointer_down chain"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

Hard-won web/mobile touch invariants (2026-06-11):

1. **Touch pointers are owned by the touch handlers.** `handlePointerDown/
   Move/Up` in TerminalPanel.ts early-return on `pointerType === "touch"`.
   Browsers fire PointerEvents alongside TouchEvents, so without the guard
   every tap double-fires (folder tap = toggle open on pointerdown + toggle
   closed on the synthesized tap).
2. **Never preventDefault a single-finger touchstart.** It cancels the tap's
   user activation in iOS Safari and the programmatic focus that summons the
   soft keyboard silently fails. Swipe-back suppression belongs in touchmove.
3. **Every `handleResize` source must deduct `keyboardInsetBottom`.** The
   MobileKeyboard insets handler shrinks the layout for the keyboard push-up;
   any other resize using raw `root.clientHeight` (e.g. the per-frame
   terminal-rect sync) silently undoes it.
4. **Agent pane clicks on web go through `agent_pointer_down(x,y)`** (wasm) —
   the desktop `handle_neoism_agent_click` priority chain (picker rows → side
   panel → permissions → links → tool cards → wordmark). Chrome's
   `handle_event` does NOT route pointer events to the agent pane.
   Wheel/drag use `agent_scroll_at` / `agent_drag_at` (picker → side panel →
   diff card under cursor → timeline; diff sign is flipped vs timeline).
5. **Taps consumed by agent UI set `agentTapConsumed`** so touchend
   preventDefaults and compat mouse events can't steal keyboard focus.
6. Chrome-side relayouts (panel toggles, Esc-close) never pass through
   handleResize — the per-frame terminal-rect fingerprint sync in `draw()`
   re-runs the resize contract (cols/rows + nvim/pty resize) when it moves.

**Why:** each was a user-visible regression (double-toggling folders, dead
keyboard, no push-up, nvim glyphs stretched under Alt+G).

7. **Soft-keyboard editor bytes must be rewritten to `nvim_input` <>
   notation** (0x7f → `<BS>`, `<` → `<lt>`, …) — the daemon passes the raw
   string to `nvim_input`, and a raw DEL byte decodes as nvim's internal
   `<80>ku` keycode (Backspace pressed Up).
8. **Tab kind, never tab index.** Chrome's `is_terminal_tab_active` keys on
   `buffer_tabs.target_at` (terminal tabs are the only targetless tabs);
   restored strips put file tabs at slot 0 and fresh terminals later. The
   old `index == 0` shortcut painted splash under markdown tabs and left
   "Terminal 2" black.

**How to apply:** any new touch surface or resize source must respect 1–3;
new agent UI interactions extend the `agent_pointer_down` chain, not new
ad-hoc exports.
