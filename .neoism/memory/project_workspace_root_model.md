---
name: project_workspace_root_model
description: Golden-standard workspace model — workspace IS a daemon-owned declared directory; terminal cd is local
metadata: 
  node_type: memory
  type: project
  originSessionId: 11179301-798e-4d64-b429-08eb0947dd9a
---

**Workspace = a declared directory, owned by the daemon.** It is the multiplayer/collaboration unit (everyone in a workspace shares the dir + its files via CRDT). The Explorer/tree/LSP/git all root at the workspace's `root_dir`, period. Tabs (terminals, open files, layout) are **per-user/per-device**, not part of the shared workspace.

**Two different "cd"s — do not conflate:**
1. Terminal `cd` → LOCAL to that user's shell. Roams freely, never moves the Explorer or other users. New shells just *spawn* in `root_dir`.
2. Re-point the workspace (`SetWorkspaceRoot`) → explicit, SHARED. Changes the dir for everyone in the workspace, broadcast via tree change.

**Daemon (`neoism-workspace-daemon/src/workspace.rs`):** `root_dir` is now always a real absolute dir, never `None`. `declare_workspace_dir()` resolves+`mkdir -p`+canonicalizes on create; `ensure_workspace_dir()` backfills legacy `None` on tree reads. `create_host_workspace` declares the dir; `set_host_workspace_root` + the `SetWorkspaceRoot { workspace_id, root_dir }` protocol message re-point it. Files served per-request scoped to `root_dir` (`resolve_request_workspace_root`); `resolve_path` now also accepts an absolute path already *inside* root.

**Web (`App.ts`):** `switchToWorkspace` roots the Explorer to `workspace.root_dir` only — no cwd derivation. `handleSessionCwd` only caches (does NOT move the tree). New terminals spawn with `cwd = workspaceRoot`. Create-workspace prompts for a dir. Per-user tabs already exist (`workspaceStrips`, persisted per device).

**Reversed from an earlier wrong attempt:** the SessionCwd "terminal cwd drives the Explorer" autochdir model was WRONG and was ripped out of the web. The daemon still *tracks*/pushes PTY cwd (`ServerMessage::SessionCwd`, `/proc` poll in `sessions.rs`) — kept for terminal-title display, disconnected from the Explorer. Desktop's own autochdir (`sync_workspace_root_from_active_pane`) was left as-is ("desktop works fine"); with `root_dir` now always populated, desktop's switch path (`daemon_host_workspace_root`) just works.

Why it was broken so long: a host-workspace was a tab-group with `root_dir: None`; on a single local daemon every workspace defaulted to the same dir, so switching never *needed* to change the tree — invisible until real per-workspace folders. Multiplayer (Yrs CRDT, presence, tailnet pairing) was already built. See [[bug_workspace_switch_tree_web]], [[feedback_desktop_vs_web_paths]].
