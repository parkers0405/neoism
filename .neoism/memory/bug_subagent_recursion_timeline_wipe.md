---
name: bug-subagent-recursion-timeline-wipe
description: "2026-07-16 audit + FIXES SHIPPED same day — runaway sub-agent recursion (yolo clobbered explore task-deny; session-level deny never enforced; no depth cap), GUI timeline wipe on session switch (visibility mask), model amnesia (512-char old tool-output truncation)"
metadata: 
  node_type: memory
  type: project
  originSessionId: da0c8dd7-b576-4f8f-b6f4-8cca272e97b1
---

Audit of three related agent bugs (2026-07-16, session ses_09445181dffeSucoBFWfOmxEic "Evaluate THYMER", gpt-5.6-sol). All root-caused; FIXES SHIPPED to working tree same day (see bottom).

**1. Runaway sub-agent recursion (24-session tree, depth 5, from 2 task calls):**
- `/yolo` (`dangerouslySkipPermissions: true` in config.json) injects global `"*": "allow"` (config.rs:455) which `AgentCatalog::from_config` merges into EVERY agent (agent.rs:28-30); `merge_json_map` OVERWRITES explore's `"*": "deny"` (same-key map collision) → explore gets the `task` tool.
- The per-child `subtask_permission` guard (`task: deny` written to SessionInfo.permission, session_actions.rs:591-627) is DEAD DATA — runtime permissions come from agent config only (session_prompt.rs:369), session.permission is never read in the tool path.
- Native `general` subagent has `"*": allow` → can always recurse even without yolo.
- No depth/breadth cap anywhere (Codex defaults: max_depth=1, max_threads=6).
- `task_result`/`stop_task` enumerate DIRECT children only (tool_runtime.rs:814-824, 904-936) → grandchildren invisible + unstoppable from parent; model truthfully saw only its 2 calls while sidebar showed the whole tree. Grandchild completions notify their direct parent (re-activating it), never the root.
- gpt-5.6-sol is documented as pathologically eager to spawn subagents (Theo's guide; openai/codex #32247 etc.).

**2. GUI timeline wipe on re-entry/sub-agent-switch (nothing actually lost):**
- All messages/parts fully persisted + fully served (no server filter; slim only strips snapshots).
- `reset_session_runtime_ui` nulls `timeline_live_trace_start` on every switch (pane/ingest.rs:965 via commands.rs:419); `timeline_message_visibility` (shared panels/agent_pane/view/timeline/layout.rs:1052-1088) then hides ALL tool/reasoning/subtask/compaction parts and all-but-trailing assistant text for every turn before live_start → whole transcript collapses to user prompts + one text chunk per turn (hidden entirely if turn ends on a tool call).
- Secondary: `timeline_history` (pagination cursors/has_older) NOT reset on switch (pane.rs:208-225) → "load older" permanently broken after switching (foreign cursor → dedupe-to-empty loop).
- Also: error/abort finalizer never appends StepFinish (provider_stream_message.rs:280-333).

**3. Model amnesia / "context cut too much":**
- `message_model.rs:12-14`: only 4 most recent tool results replayed (≤12KB); ALL older tool outputs truncated to 512 chars on EVERY request. Top cause of losing sub-agent results mid-conversation. "Context 13% / Input 27,571" after reload = this truncation, not data loss.
- Prompt is rebuilt from store each step (stateless) — no reload-specific drop; orphan function_call pairing is correct.
- Codex-OAuth clamp: 400k ctx / 272k input for gpt-5.6 (provider_catalog.rs:21-22, :602-654) + token totals include reasoning tokens → compaction fires early on codex path (didn't fire in this session).

**FIXES SHIPPED (2026-07-16, working tree, all cargo check clean):**
- yolo rework: removed `*: allow` injection from config load; `dangerouslySkipPermissions` now auto-grants ASK-class permission errors in execute_tool_call_with_permission_wait (tool_runtime.rs) — asks skipped, denies preserved. Also fixes yolo-not-skipping-external_directory (agent-level ask rules out-ranked the injected `*`).
- SessionInfo.permission now appended after agent rules in session_prompt.rs (last-match-wins) → subtask task/todowrite denies enforced + task stripped from child tool lists (also stops ULTRA_DELEGATION_INSTRUCTIONS injection into children).
- MAX_SUBTASK_DEPTH=3 backstop in task tool; descendant_sessions() BFS helper; stop_task + task_result now cover whole descendant tree (nested flag), stop-by-id also stops the target's subtree; ensure_child_task_belongs_to_parent walks ancestors.
- message_model.rs: task/task_result/background_task_result outputs exempt from 512-char old-truncation (12KB recent cap instead; not during compaction requests).
- StepFinish appended on error/abort finalizer (provider_stream_message.rs).
- timeline_message_visibility FINAL SEMANTICS (user-corrected twice, 2026-07-16): settled turns hide reasoning/tools/subtask/compaction EXACTLY like the original design; the ONLY change is User+Assistant text rows are ALWAYS visible (old trailing-text-only rule deleted). User rejected both "show everything" and "collapsed one-line tool cards" — wants clean prompt+answer history, zero tool rows. A tool_archived collapsed-card mechanism (AgentToolPane::tool_archived, measure-key tool_archived bit, minimal header-only cards) was built and left in place but is INERT (mask hides archived tools before they render); reusable if a "show tools in history" toggle is ever wanted.
- timeline_history reset on session switch (commands.rs).
- Live-trace window is now VISIT-STABLE (user request, opencode parity): `timeline_live_trace_anchor` (user-message id) added beside the index on both panes; rebase_current_turn_trace re-derives from the anchor instead of jumping to the latest turn — sending a new prompt no longer collapses earlier turns' trace; ONLY leave-and-return collapses. Empty-id (optimistic) anchor = sentinel → falls back to latest-turn + re-anchors on durable id. note_timeline_prepend already shifts the index (do NOT also rebase in apply_older_timeline_page — double-shift).
- Also fixed 3 pre-existing test-fixture compile breaks so neoism-ui tests run (MarkdownPane missing icon/cover fields ×2, PaletteWorkspaceEntry.current); desktop_fork_guard file_tree test fails at clean HEAD (pre-existing).
- Pre-existing broken test targets (NOT mine): agent-server examples/completion_probe.rs + 3 lib tests (mcp_memory vault, notes tool, compacted_summary_trims); neoism-ui markdown/palette fixtures missing icon-cover fields; desktop context/manager tests (per-window WIP).

Related: [[bug-codex-limit-drift-compaction]], [[perf-agent-pagination]], [[feature-prompt-picker]].
