---
name: "Sidebar viewed-buffer highlight synchronization"
description: "Passive LEFT follows viewed owning root; RIGHT follows exact viewed parent/child, same-family scroll retained. Native focus+web split restoration wired; 55 shared+1 native+4 web tests and checks passed."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Sidebar highlight follows the viewed conversation buffer

Both left catalog and right details painted background from keyboard selected_index even when unfocused. Viewed IDs only changed dot/title accent. Right sync also cleared roster, reset selected=0 and content scroll whenever exact viewed session changed, including parent/child in same family.

Shared side_panel.rs now exposes set_catalog_viewed_root(Option<&str>): once bound, explicit None is authoritative and no cached pane ID can replace it. Passive/unfocused catalog background resolves this owning root; passive detail background resolves exact viewed parent/child. Focused keyboard navigation/caret still uses independent selection; missing/paginated rows have no incorrect fallback highlight. Native host/run.rs passes owning root from focused agent context (child maps root; file/terminal clears). Shared Chrome draw binds actual active BufferTabTarget kind/root, not retained pane identity when viewing files/terminal.

sync_conversation_details_from_root compares family identity rather than exact viewed ID. Parent -> child -> sibling -> parent preserves detail roster/viewport/navigation cursor; different family resets. Viewed historical-child retention and terminal locks remain. Historical child test now checks semantic BranchStatus to accommodate existing stopped->error canonicalization without changing lifecycle behavior.

Web split activatePaneExternalId previously changed tab chrome/editor surface but never switched the agent transcript. Both bound and fallback agent paths now activate correct tab; rememberActiveAgentSession saves any child view on source tab before switching. Same-session focus avoids redundant switch/jump, and repeated focus of already-open draft avoids clearing it. Rust agent_session_id now returns actually displayed shared pane session rather than lagging daemon-ack bridge scratch value. Four Node tests exercise actual TerminalPanel prototype methods with mocked platform effects, including A/B focus, retained child, no reset and file/fallback paths.

Verification: shared view::side_panel::tests 55 passed; desktop catalog_view_maps 1 passed; Node agentPaneFocus 4 passed; npm typecheck, native/shared cargo check, wasm32/web cargo check and whitespace check pass. No release build or live GPU visual validation. Existing activity colors, tool/fog animation, timeline physics, sidebar pagination and focused keyboard browsing preserved. Related: [[feature_multi_agent_conversation_sidebar]], [[feature_family_catalog_activity_colors]], [[bug_viewed_subagent_disappears_on_completion]].
