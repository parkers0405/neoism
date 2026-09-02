---
name: "Desktop sub-agent live view and parent staleness"
description: "Desktop child views buffer pre-discovery events, keep parent state current, and show the complete loaded reasoning/tool history for ongoing sub-agents."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-19"
---

# Desktop sub-agent live view and parent staleness

## OpenCode v2 invariant
OpenCode keeps one global event stream, buffers unknown child events until discovery, continuously reduces parent and child state independently of selection, and renders every accumulated child `commit` in the running inspector. Selecting a child changes presentation only.

## Neoism root causes fixed
- SSE classifier copied known child IDs only at connection startup, dropping restored/nested child events discovered later.
- Child completion updated only the currently displayed transcript, leaving the cached parent task card running.
- Warm switches could retain an unrelated family root; same-family tree results could be discarded after navigation.
- Switching cleared `timeline_live_trace_start`; opening an already-running child therefore hid its already-emitted reasoning/tools/edits/subtasks until another event arrived.
- Pagination shifted a running child's reveal-all boundary, hiding older tool history as it loaded.

## Implemented behavior
- Root SSE refreshes family membership before each event and buffers 512 unknown-session events for replay after discovery.
- Lifecycle status updates every active/cached transcript containing the owning task card.
- Terminal child edges schedule a deduplicated root snapshot repair.
- Family root replacement is deterministic and same-family refresh results survive navigation.
- Warm activation and cold runtime hydration immediately set ongoing sub-agent history to reveal from index 0. This includes every loaded turn's reasoning, tool calls/results, edits, subtasks, text, and errors. Completion keeps it visible for the current visit; reopening a settled child returns to the normal clean history mask.
- Loading older pages while the child is still ongoing keeps the reveal boundary at 0, so older tools remain visible.

Initial child hydration fetches the latest 100 message blocks. Older blocks remain available through existing timeline pagination and now stay visible for an ongoing child.

## Key files
- `neoism-frontend/desktop/src/neoism/agent/updates.rs`
- `neoism-frontend/desktop/src/neoism/agent/commands.rs`
- `neoism-frontend/desktop/src/neoism/agent/pane/{ingest,session,render_state,tests}.rs`

## Validation
Shared classifier regression passes; changed files have no error diagnostics and `git diff --check` passes. Full desktop Cargo validation remains blocked by the unrelated missing `neoism-agent-server/src/workflow.rs` module.
