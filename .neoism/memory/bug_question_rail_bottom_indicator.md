---
name: "Question rail bottom indicator"
description: "Bottom/live-edge rail selects LAST question rather than first visible; shared test compilation passed, full host checks blocked by unrelated provider/catalog work"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-24"
updated: "2026-10-24"
---

User reported at-bottom question rail highlighted an earlier marker when multiple short questions fit in viewport. Root cause: selection always preferred first visible question even at live edge. Fixed in shared `panels/agent_pane/question_rail.rs::active_question_index`: explicit at_bottom selects last target. Renderer passes at_bottom = timeline_follow_bottom() || max_scroll - scroll_top <= s, covering follow mode, manual bottom scroll, no-overflow and subpixel residuals. Reading older history retains first-visible/preceding behavior. Regression `bottom_selects_last_question_when_several_questions_are_visible` covers clustered questions, older-history selection, follow override and empty targets. Shared renderer fixes desktop/web.

Verification v0.7.136: cargo check -p neoism-ui --tests passed. Full desktop check blocked by unrelated concurrent provider_service.rs lines88/126 method argument mismatch; WASM blocked by rendered/catalog.rs line744 missing catalog_status/request_id in pattern. Did not modify these unrelated in-progress files. No app launched or tests executed.
