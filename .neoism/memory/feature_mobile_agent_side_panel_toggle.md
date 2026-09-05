---
name: "mobile agent side-panel toggle"
description: "Shared Rust mobile-only Agent side-panel toggle, takeover, and close-on-chat-navigation behavior"
type: "feature"
scope: "project"
origin: "coding task"
created: "2026-08-19"
updated: "2026-08-19"
---

Mobile-only Agent side-panel toggle/takeover is shared-Rust authoritative. WASM enables `Chrome::set_mobile_web_agent_panel_enabled`; narrow breakpoint is Agent `SIDE_PANEL_MIN_PANE_WIDTH`. Topbar shows a separate 44px hit target beside the persistent global Agent button. Open panel fully takes Agent content and suppresses timeline/composer interaction; resize wide restores prior desktop panel state.

Extension: `NeoismAgentPane::activate_side_panel_row(showing_sessions, dismiss_after_navigation)` centralizes session/subagent/current-session navigation. WASM keyboard Enter and pointer-row activation pass `Chrome::agent_side_panel_takeover_active()` as dismissal policy. On successful navigation the existing `switch_session` authoritative action runs, home override/focus clear, and narrow takeover immediately sets `user_hidden=true` plus relayout. Wide/desktop pass false/retain panel. Search focus and other non-navigation actions do not close. Semantic excerpt rows and root/child subagent rows use the same path. Immediate hide occurs before async switch acknowledgement, so failures cannot leave takeover trapping the chat.
