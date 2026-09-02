---
name: bug-agent-pane-web
description: "Web agent pane failure modes — invisible streamed text (virtual timeline revision), double CreateThread, one-shot agent-server spawn, boot picker eating keys"
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

Four web agent-pane failure modes diagnosed/fixed 2026-06-10:

1. **Streamed text invisible while taking up space**: shared `state.rs` dirty-mark paths (`mark_timeline_message_dirty_at`) deliberately do NOT bump `timeline_layout_epoch` (the view layout cache patches in place), but `sync_virtual_timeline` rebuilt only on epoch change — so the virtual surface kept the pre-stream message list and its `visible_source_range` culled every new row. Fixed with `timeline_content_revision` (bumped on every message mutation; virtual timeline chases it).
2. **Double CreateThread fork**: one Enter with no session drains `EnsureSession` + the pending-prompt arm; both called `create_agent_thread_with_defaults` → turn ran on one session, pane bound the other, stream filter dropped everything. Fixed with `thread_create_inflight` single-flight in wasm `AgentBridgeState` (reset on ThreadCreated/Error/Disabled).
3. **Agent server never found**: daemon's `ensure_agent_server_started` was a one-shot OnceLock; "Address already in use" (desktop owned 4096) wedged it forever even after the desktop exited. Now a supervisor loop: health-probe → listen when unhealthy → retry every 2s.
4. **All keys eaten at boot**: e4f9ec1a boots web into the Workspaces picker; on some paths the modal is *visible in chrome state but unpainted*, and `keyboard_capture_active()` swallows every key until Escape. Owned by the workspaces-picker flow, not fixed here.

**How to apply:** when web agent symptoms look like "state right, render wrong", dump pane state vs screenshot; wire-capture `{"AgentReply":…}` frames with Playwright `page.on("websocket")`. Daemon history/live parts now map through shared `api_mapping::message_blocks_from_response`/`part_block` (desktop parity, `order=desc`). See [[bug-stale-session-attach]].
