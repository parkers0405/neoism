---
name: "Conversation search: closest matches, then recent activity"
description: "Sidebar says Search; full-coverage/phrase relevance first, bounded 5% root-activity recency bonus; keep frontier upper bound correct"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-18"
updated: "2026-10-18"
---

# Search label and closest/recent ordering

The agent sidebar label is exactly `Search`, not `Search conversations`; the modal can retain its descriptive title. Shared sidebar label lives in `neoism-frontend/shared/src/panels/agent_pane/view/side_panel/draw.rs`.

Conversation ranking in `neoism-agent-server/src/state/conversation_search.rs` keeps lexical relevance primary. Native query construction preserves OR recall but boosts complete term coverage (AND clause) and exact word-order phrases. Conversation activity supplies a bounded 5% maximum recency bonus, halving after 30 days, based on the authorized root conversation's `time.updated`, not just the matching message's creation date. Equal ranked scores break ties by root activity, then matched-message creation time and stable IDs. An old exact phrase still beats a recent scattered or partial mention. Strict message/tool AND queries retain their prior behavior.

IMPORTANT: adaptive relevance-frontier termination must account for BOTH maximum role boost and maximum recency boost (1.25 * 1.05 for conversation mode); otherwise unseen recent/user hits could be omitted. No SQL shape/index changes or body loads were added. Regression `conversations_prefer_closest_words_then_recent_activity` checks exact vs scattered vs partial ranking, equal-relevance recent-first order, and activity updates independently of old matching-message timestamps. Verification: all 35 search tests passed and `cargo check -p neoism-ui --tests` passed.
