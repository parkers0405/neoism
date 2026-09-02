---
name: "Closed sidebars stay closed during focus navigation"
description: "Alt+Left now skips a closed file tree in desktop and web while preserving explicit toggle behavior"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-31"
---

Alt+Left/Right chrome focus traversal must navigate only through visible panels. Closed file tree must not auto-open from editor, split boundary, first workspace tab, or first Island tab. Fixed in desktop `screen/panes/close_focus.rs` and shared/web `chrome/events.rs`; hidden-tree boundaries preserve current focus. Explicit Alt+E/toggle behavior remains unchanged. Verified with `cargo check -p neoism-ui -p neoism` and targeted rustfmt check.
