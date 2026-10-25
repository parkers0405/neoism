---
name: "Subtle subagent session navigation"
description: "Subtle Back/Prev left + Next right in child header; ordered retained-family navigation, root/last no wrap, direction-safe hits, cached drafts preserved"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

Subagent transcript header now provides subtle ordered family navigation. Left is `\u{2190} Back` when previous destination is the main/root chat, otherwise `\u{2190} Prev`; right is `Next \u{2192}` when another retained child follows. Uses CURRENT root-first sidebar roster order including nested descendants, NOT history or direct-parent traversal. No wrap; last child has no Next. Root/main still has no header row. Missing viewed child/untrusted roster falls back to Back to known root only. Expired siblings are skipped via side_panel.visible_subagents(); viewed completed child remains navigable until existing lifecycle prunes it after leaving.

Visuals shared view/chat.rs: 12px-scaled body font, theme.muted normally, theme.fg on hover, no filled background/border, sticky28px-scaled row above timeline. Left/right align to chat-column edges with separate hit/hover regions. Omit Next on narrow columns rather than overlap; tiny panes omit header, hits cleared even before early render returns. Empty child transcripts render chat/header before messages arrive. Root/home layout unchanged.

Shared conversation_header.rs: Direction::{Previous,Next}, borrowed ConversationNavigation {previous,next,previous_is_main}, target(direction); navigation_targets helper validates family root and derives adjacent IDs. ConversationHeader stores two frame-local links. clear() resetsboth; register(Direction,target,rect,clip,occlusions) updates ONLY one direction; hit_at returns HeaderHit {direction,target}. Shared/native panes expose conversation_navigation(), conversation_navigation_at(x,y), activate_conversation_navigation(x,y). Click revalidates destination for the SAME direction (not either neighbor), rejecting stale hits after roster reorder even when target becomes opposite neighbor. Picker/modal occlusion wins; navigation clears header then uses existing cache-aware switch_session in same pane.

Native bridges/agent.rs, app/window_event/mouse.rs, pane/render_state.rs handle clicks, cursor and hover. WASM rendered/agent.rs and frame.rs use same generic activation and existing outbound drain. No new tabs/routes/protocol; SwitchThread/GetHistory/ResumeStream update session_id/requested_session_id gating. Both native/shared CachedAgentSession retain input:String via cache park/restore so unsent root draft survives Prev/Next child visits; transcript/permissions/roster/runtime caches remain.

Tests: shared state/conversation_header_tests.rs and view/chat.rs::header_tests; native pane/tests.rs::conversation_navigation*; wasm/tests/conversation_back.rs (filename retained, now6directional contracts). Focused shared navigation26tests, branch6tests, web6tests pass. Broader shared suite has11 unrelated failures from prior full run; do not claim all shared tests pass.
