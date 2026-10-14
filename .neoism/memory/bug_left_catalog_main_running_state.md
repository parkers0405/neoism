---
name: "Left Conversations main-session running state"
description: "LEFT Alt+C independent catalog dropped root session.status; now scoped SSE busy/idle updates survive metadata/snapshot races. Follow-ups use continuation arrow, no duplicate spinner. Native route tests pass."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

# Left Conversations sidebar did not track an active main session

The first square-spinner patch matched sidebar visuals but did not repair native left catalog data flow. User clarified LEFT Alt+C Conversations pane, not the right branch/detail pane, must indicate active main sessions. Native `renderer.conversations_pane` is an independent NeoismAgentPane with a workspace catalog subscription, not a selected conversation-family stream. Catalog SSE originally admitted only root create/update/delete, so `session.status` busy/idle edges never reached it. Ready catalogs do not periodically replace the first page, which preserves pagination/scroll but makes the missing event path permanent.

Fix: agent-server `v2_session_catalog_events` also admits SESSION_STATUS. Resolve SessionInfo by event session ID, then retain the existing hydrated-host/authorized caller/directory/root-only filters; child or foreign-directory activity is not forwarded as root rows. Native updates.rs parses canonical SSE into `AgentSessionCatalogUpdate::RuntimeStatus { session_id, running }`; ingest.rs applies it to the catalog even when no conversation event stream exists. Shared side_panel.rs mutates both stored/display root rows in place without selection, sorting, or scroll reflow. A per-ID live override prevents metadata-only title/rename and in-flight snapshots from clearing running or resurrecting idle; clear overrides on reconnect before authoritative hydration, on deletion, and directory invalidation. This is not a right-side branch lifecycle mutation.

Follow-up correction: title-only labeling was insufficient for user. Shared tool_message/render.rs now paints a muted continuation arrow (`↳`) for task Follow-up rows, including bare Follow-up title. No incoming/outgoing lifecycle dot/spinner and no running-tool redraw claim. Launch Task rows keep normal status/spinner. Chronological IDs, navigation targets, entry/fog/status-label transitions, reserved geometry and git-diff header targets are unchanged.

Verification completed exit 0: left_catalog shared 3 passed; followup shared 6 passed; native left_catalog 1 passed using real canonical SSE decoding -> catalog queue -> drain_live_session_updates on separate pane with no chat stream; agent-server v2_session_catalog_stream 2 passed including actual HTTP root busy/idle and child/foreign-directory exclusion. Native/shared/daemon/server cargo checks, wasm32/web check and whitespace check pass. Earlier shared timeline cfg-test argument errors were concurrent transient edits and had resolved by this run. No release build or live GPU visual validation. Related: [[feature_task_followup_and_sidebar_running]], [[feature_multi_agent_conversation_sidebar]].
