---
name: bug-stale-session-attach
description: "Web terminal dead (no composer, keys go nowhere) when attach binds a stale session id from the persisted host workspace tree"
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

The daemon persists the host workspace tree (tabs carry `session_id`) across restarts, but live PTYs die with the process. `tryAttachExistingWorkspaceSession` / `switchToWorkspace` in `web/src/app/App.ts` bind the panel to whatever session id the tree offers; if it's dead, every Resize/PtyInput bounces with `unknown session <id>` and the terminal sits dead — no OSC 133 prompt marks, so the shared `>>>` command composer never shows (`composer_footer_active` needs `awaiting_command`).

**Why:** the composer's visibility is driven entirely by OSC 133 semantic-prompt marks parsed by the wasm terminal; a dead session means no bytes at all, which looks like "composer missing" but is really "no shell".

**How to apply:** fixed 2026-06-10 via `handlePtyError` in App.ts (matches `unknown session`, calls `TerminalPanel.respawnDeadPtySession` which drops the dead tab and respawns through the pending-spawn queue; `staleSessionRecovery` set dedups, cleared per connection). Debug trick: `feed_pty_output` with `\x1b]133;A\x07\x1b]133;B\x07` from the page flips the composer on if the wasm side is healthy — isolates wasm logic vs byte-stream problems. See [[desktop-vs-web-paths]].
