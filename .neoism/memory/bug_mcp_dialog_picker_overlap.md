---
name: "MCP dialog overlaps inline picker - fixed"
description: "MCP OAuth modal background looked invisible because retained inline picker sorted over it; dismiss picker before agent dialog event"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

Desktop MCP OAuth used to leave the `McpActions` inline picker alive when `system_message` queued a modal. Picker material order (~180) sorts above modal material (~24), and both widgets' labels enter the late overlay text batch, so the picker covers the modal background and glyphs overlap. Raising modal shape order alone cannot fix label bleed. Fixed the agent dialog handoff in `neoism-frontend/desktop/src/neoism/agent/pane/ingest.rs::push_dialog`: after rejecting blank bodies, set `self.picker = None` before queuing the host Dialog event. This removes picker material, text, hit geometry, and published occlusion before the host paints the pane. Do not call `close_picker()` for this handoff: MCP action Escape navigates back to/re-fetches the MCP server picker. Short notices retain picker for retry. Regression tests in pane/tests.rs cover Slash/Mcp/McpActions handoff, draft preservation, stale geometry, empty dialog and notice behavior. `cargo check -p neoism --tests` passed; no live UI verification or test execution. Shared/web OAuth currently appends transcript URL rather than opening this desktop dialog; left unchanged.
