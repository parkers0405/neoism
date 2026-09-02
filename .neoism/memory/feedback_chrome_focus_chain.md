---
name: Chrome panels participate in the Alt+arrow focus chain
description: New chrome panels (file_tree, git_diff_panel) need to claim a slot in `focus_horizontal_chrome` so Alt+Left/Alt+Right cycles between them and the editor like the user expects.
type: feedback
originSessionId: 97126cba-f109-488f-9a2f-2b00efd4a19c
---
When adding a chrome panel, it must integrate with the global
`Screen::focus_horizontal_chrome(right: bool)` chain so Alt+Left /
Alt+Right ↔ editor / panel navigation feels symmetric to the file tree.

**Why:** The user explicitly compared a new panel's focus model to the
file tree's: clicking the entrypoint should land focus *on the panel*,
and Alt+arrow should walk in/out without a mouse round-trip. Anything
short of that "feels broken."

**How to apply:**
- Add a `focused: bool` to the panel + `is_focused()`/`set_focused()`.
- `open()` sets `focused = true`; `close()` clears it. Clicks inside
  the panel body promote focus too.
- In `focus_horizontal_chrome`, handle the new panel *before* the
  existing file_tree / split-pane logic so the chain stays symmetric:
  - If panel focused and `!right` → `set_focused(false)` + return to
    main workspace.
  - Else if `right` and panel `is_visible()` → `set_focused(true)`.
- Add a per-panel `handle_*_key` that consumes ↑/↓/j/k for selection,
  Enter for activation (e.g. `:edit` the selected file), Esc to close.
  Route to it from `process_key_event` *before* the file_tree key arm,
  gated on `panel.is_focused() && state == Pressed`.
- When a click lands outside the panel rect, defocus it so the editor
  underneath gets keyboard focus on the next keypress.
- Show a focus-state visual cue: brighten the selection accent stripe
  when focused, dim it when unfocused, so the user can tell at a
  glance whether arrow keys will move the panel's selection.
