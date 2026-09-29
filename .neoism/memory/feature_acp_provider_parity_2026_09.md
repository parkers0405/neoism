---
name: "ACP provider parity and sidebar"
description: "ACP provider parity and sidebar fixes"
type: "feature"
scope: "project"
origin: "Current coding session 2026-09-25"
created: "2026-09-28"
updated: "2026-09-28"
---

2026-09-25 nightly workspace changes (uncommitted as of this note): ACP external providers OpenCode/Claude Code/Codex title & event parity pass. Desktop agent tab title now bare session title; provider icon stays separate. External first prompt and imported replay derive default session title; OpenCode native catalog default titles derive from first local SQLite user part; v2 authorized roots list lazily repairs old default external titles from stored user parts using full SessionInfo (never persist compact list sidecar!). ACP command discovery accepts leading slash names (normalizes for UI), skips invalid/duplicate optional entries rather than 400, preserving valid model options. `external_agent/events.rs` carries forward tool kind/title/input over sparse `tool_call_update`; explicit ACP diff content maps to `acpDiffs` and shared UI edit cards, Codex spawn_agent message maps to nested child only with explicit task shape. ACP terminal/create/output/wait/release project synthetic bounded chat tool parts, excluding env fields, with running output via top-level tool metadata; never infer historical tools. Left desktop conversations sidebar right-click Rename/Delete uses existing context menu and modal, scope-check server+workspace, confirms delete and refreshes open tabs. Date headers use `theme.readable_accent(theme.blue)` matching composer model label. `cargo check -p neoism -p neoism-ui -p neoism-agent-server` passes; 38 server ACP tests pass. Broad shared agent-pane suite: 590 pass, 3 existing failures in untouched mobile navigation/chip tests (mobile_semantic_search_result_navigation_dismisses_takeover, mobile_child_and_root_navigation_both_dismiss_takeover, tab_mode_switch_rearms_agent_chip_transition). Real adapter emitted payloads and web right-click not end-to-end validated; historical provider imports remain text-only.
