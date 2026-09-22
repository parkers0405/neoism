---
name: "Joined ls spinner: adoption skips CreatePty"
description: "Actual UI join silently skipped CreatePty because adoption checked outgoing grid ownership; reproduced and fixed with direct known-owner creation"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-22"
updated: "2026-09-22"
---

2026-09-22: Reproduced user's ls infinite spinner via actual desktop top-right server menu joining isolated ws://127.0.0.1:19879/session. Prior raw-PTY two-client tests passed but UI join FAILED. Root cause in desktop/context/manager/daemon_sessions.rs: adopt flow calls register_remote_context_with_cwd(&root_context, root) BEFORE installing adopted grid/current_index/adopted_workspaces binding. Guard current_workspace_uses_attached_daemon sees outgoing local grid while link_is_peer=true, returns false; no current_adopted_workspace_endpoint means it silently returns without CreatePty or pending retry. Pane queues commands forever, no shell exists. Fix extracts create_remote_context_on_attached_daemon and calls it directly for adoption's fresh root (attached owner already known); ordinary pane creation keeps owner guard/defer. Rebuilt cargo build -p neoism; repeated exact UI menu join: link icon visible, ls output present, green completion timer 0.037s. Tab fallback in manager/navigation.rs uses daemon declared root when terminal-derived title empty or '~'; actual linked tab displayed neoism. Prior subagent-only fixes (socket timeouts, sequenced reconnect replay, bounded subscriber channels) did not fix this creation bug. Always test desktop server-menu join, NOT startup --daemon-url local terminal and NOT just daemon websocket fixtures. Debug PID468275 current test client and PID445229 host isolated; no nightly deployed. Automated remote_pty_io all3 passed, web WorkplaceService19 passed, npm typecheck and debug build passed before final root helper edit (desktop rebuild passed afterward).
