---
name: project_shared_notes_agent_scoping
description: "Making notes + agent workspace-scoped (shared) for the HOST too, not just guests — self-host and docker symmetric"
metadata: 
  node_type: memory
  type: project
  originSessionId: 61dc832e-208d-41c5-a0b4-dbc86d0b1599
---

**Goal (user):** notes + neoism-agent should be SHARED across host and guest, working identically whether self-hosted (laptop spawns a daemon) or docker-hosted (pod). Model: the workspace (files, notes, agent) lives with the daemon/pod; everyone — including the host — is a client of it.

**Root cause of the host/guest asymmetry:** two different signals were conflated.
- `current_workspace_is_remote_joined()` (daemon_link.rs:183) = host-IDENTITY (`workspace.host_id != local_host_id()`, and `local_host_id = desktop_host_id(window_id)`). **False when you host your OWN workspace.**
- `daemon_link_is_peer()` (daemon_link.rs:337) = connection-TYPE (linked to a non-home daemon). **True for self-host, docker, AND guest.**
Notes + file-tree-remote-mode keyed on the host-identity flag, so a self-hosting host fell through to its PERSONAL local vault while guests read the (empty) workspace `Notes/`. The self-host corner is where `link_is_peer=true` and `remote_joined=false` disagree — `markdown_crdt.rs:~222` even treats that combo as a "desync". Files still multiplayer for the host via the shared FILESYSTEM (local edits + daemon serving same dir + autoread/checktime convergence), which is why `remote_root()` is None for the host (`sync_file_tree_remote_mode` gates on remote_joined).

**Agent — already correct, no routing change:** `complete_server_switch` (app/mod.rs:1166) re-points agent panes to the workspace agent (`agent_server_for_daemon_endpoint` = daemon-port+1) for any `!is_home()` session, which includes the host on its own `127.0.0.1:9877`. So host+guest both hit the workspace agent once the binary has this code. The user's earlier "host=personal / guest=empty" was the pre-rebuild binary (guest wasn't routed to the host agent yet).

**Agent store is SHARED by construction on self-host (turso):** conversation DB = `default_state_dir()`/`agent.turso.db` = `$HOME/.local/state/neoism/agent.turso.db`, keyed by directory. NO per-port/instance suffix. `db_backend_from_env()` (state.rs:119) defaults to **Turso** (only `NEOISM_AGENT_DB_BACKEND=sqlite` opts out → separate `agent.sqlite3`, "starts from empty history"). The Create-Server-spawned `neoism-agent serve --port {port+1}` inherits the host's HOME with no data-dir override, so it opens the SAME turso file as the local agent → guest joining `~/Github/synapse-ai-hub` sees the host's existing synapse chats; a new dir = empty (no chats for that path key yet); docker = the pod's own isolated turso file = fresh. Two processes on one turso file is handled by `turso_busy_retry` (state.rs). **Landed:** pinned `.env("NEOISM_AGENT_DB_BACKEND","turso")` on the spawn (palette.rs) so a stray sqlite env can't silently split the store. User policy: ALWAYS turso, never sqlite (FTS5 is sqlite-only → turso message search falls back to LIKE scan).

**Notes fix (LANDED, cargo-check clean on branch better_workspace, needs runtime verify on next build):** added `served_workspace_root()` (daemon_sync.rs) = `daemon_link_is_peer()` guard, prefer `file_tree.remote_root()` (guest) else `current_adopted_workspace_id()` → `daemon_host_workspace_root()` (daemon_sessions.rs:327, reads `workspace.root_dir` — valid even when the tree is LOCAL). Added `send_remote_files_op_with_root(root, msg)` (explicit-root files-plane op). Rewired all notes sites to `served_workspace_root()` instead of `current_workspace_is_remote_joined()`: open (notes_create.rs), refresh + create-target + create (sidebar.rs x3), notes menu (notes_menus.rs), grid auto-open (grid_workspace.rs), and the `FileCreated` note-open arm. **Deliberately left file-tree / CRDT / git / search on the old flag** — zero regression risk to the working editing multiplayer.

**Runtime dependency to verify:** for the self-hosting host, `served_workspace_root()` needs `current_adopted_workspace_id()` = Some AND `daemon_host_workspace_root()` to resolve. If the host's OWN served workspace isn't "adopted" in the client sense, it returns None → host still sees personal notes → then the remaining fix is to make the Create-Server/host flow ADOPT the served workspace. Check this first on the next build.

Related: [[feature_per_window_servers]], [[project_workspace_root_model]], [[bug_fs_watch_node_modules_storm]], [[project_notes_sidebar]], [[project_agent_user_part_sync]].
