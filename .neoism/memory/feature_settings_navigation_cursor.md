---
name: "Settings navigation cleanup and hover cursors"
description: "Hide empty Platform, group Renderer under Developer without config migration, shared clickable/text/default cursor ownership for native and web Settings."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

Settings GUI category navigation in shared/src/panels/settings_page/state.rs uses navigation_categories(): omit Platform only when no retained visible descriptors belong to it. Renderer GUI category removed; ConfigCategory::Renderer maps to GUI Developer, preserving all renderer.* config paths and protocol categories. Both desktop rail and compact category list use this helper including compact scroll metrics. pointer_kind_at classifies painted settings hit regions: Pointer for navigation/toggle/select/font/action/keybind/reset/header buttons, Text for search/value/custom-value editors, Default for inert overlay space. Open-dropdown classification takes precedence; stale detail targets clear during navigation/descriptor replacement. Desktop previously had unused handle_settings_hover; it now returns Option<CursorIcon> and owns pointer motion and active-settings release, and cursor is refreshed after settings click. Browser frame.rs queries same shared classifier before generic overlay fallback. TerminalPanel refreshes both canvas and markdown-layer cursor after page/picker overlay presses; touch exclusion preserved. Tests: 22 settings Rust tests and 6 web pointerCursor tests pass; native cargo check -p neoism and npm run typecheck pass. No live UI launched.
