---
name: "Multiline input wheel scrolling"
description: "Wheel/trackpad scroll over wrapped agent and terminal input; persistent viewport, caret reveal on edit, desktop/web wired"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-11"
updated: "2026-10-11"
---

Agent input and terminal composer now accept vertical wheel/trackpad gestures over their rendered text regions. Both preserve a manual first-visible-row across idle redraws, accumulate sub-row trackpad deltas without inertia, clamp/consume at limits, and reveal the caret again on actual text or caret changes. Text snapshots detect same-length edits. Offscreen caret/terminal ghost suggestion is suppressed. No new scrollbar/chrome was added.

Agent: shared `panels/agent_pane/input_viewport.rs` is the persistent policy, stored separately by shared state.rs and desktop neoism/agent/pane.rs. `view/user_input.rs` bridges via AgentUserInputPane::input_viewport_mut, publishes logical text geometry, and renders from persistent row offset. Desktop overlay_scroll.rs resolves hovered split before agent routing, accounts for device scale for pixel deltas, and leaves modal/palette/finder/context/diagnostic priority intact. Wasm rendered/agent.rs wires both pixel and device-normalized wheel entries. Prompt-picker overlap consumes without scrolling draft.

Terminal: `panels/command_composer/text_viewport.rs` handles its chrome-only first slot while scrolled. render.rs publishes ComposerFrame::text_viewport_rect and uses observe_input exact snapshot/cursor. update.rs scroll_text_at excludes send chip and invalid/nonvertical deltas. Desktop host/composer.rs plus window_event/scroll.rs intercept before terminal output; shared chrome/events.rs handles generic events and direct wasm wheel bridge. Inactive terminal composers are still existing transient previews; active composer owns persistent text state.

Verification: cargo check -p neoism -p neoism-ui and wasm32 check -p neoism-terminal-wasm --features web passed. Eight standalone viewport policy tests passed (four per helper). Shared --tests compilation reached lib tests but failed existing shared/tests/buffer_tabs.rs:289 missing PanelContext.plugins. Additional pane/command state tests added/typechecked with lib tests. No release build or app relaunch.
