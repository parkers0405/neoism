---
name: feature-ssh-follows-tree
description: "Terminal ssh auto-flips the file tree to the remote host's disk (v1: browse+read); reuses the joined-workspace RemoteFileSource seam over system ssh ControlMaster"
metadata: 
  node_type: memory
  type: feature
  originSessionId: 7f62545c-19db-444c-86e4-3b6a6574bcc7
  modified: 2026-07-28T20:35:28.141Z
---

**2026-07-28, uncommitted (user tests before commit).** Typing `ssh [user@]host` in a terminal pane auto-flips the file tree onto that host's filesystem; `exit`/`logout` restores the local tree. Zero remote setup — shells out to system `ssh` (respects ~/.ssh/config, keys, agent) with ControlMaster multiplexing.

**Architecture — reuses the joined-workspace tree seam:** the tree's remote backend was generalized from concrete `Arc<RemoteFiles>` to `Arc<dyn RemoteFileSource>` (trait in `editor/file_tree/state.rs`: `FilesService + root() + request_read_file() + as_files_service()`). `RemoteFiles` (daemon) and new `SshFiles` (`daemon_client/ssh_files.rs`) both impl it. SSH backend **synthesizes the daemon's own `FilesServerMessage::DirListing`** so replies flow through the identical `apply_daemon_files_message → handle_service_reply` path.

**Reply round-trip (no runtime handle on this backend):** `list_dir` → `Err(IoError::Pending(id))` → detached `std::thread` runs `ssh -S <sock> ls -Ap -1 --color=never` → parse (trailing `/` = dir) → `reply_tx.send((id, DirListing))` + `event_proxy.send_event(RioEvent::SshFilesReady)` (new UNIT variant — neoism-backend can't dep neoism-protocol) → app drains Screen's mpsc via `drain_ssh_files_replies` → `apply_daemon_files_message`. Screen fields: `ssh_files_tx/rx`, `ssh_files_next_id`, `ssh_pre_local_root`.

**ssh commands:** master = `ssh -f -N -M -S <path> -o ControlPersist=180 -o BatchMode=yes -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new <opts> <target>`; list/read reuse `-S <path>`; Drop = `ssh -S <path> -O exit`. **BatchMode is load-bearing** — key/agent auth = silent; password-needing host degrades to empty tree, never hangs the UI.

**Detection:** hook at `screen/lifecycle/block_overlay.rs` right after the passthrough toggle (`entering_passthrough`/`leaving_passthrough` from the typed command). `parse_ssh_target(cmd)` (in screen/mod.rs): basename must be `ssh`; value-flags (`-p -i -o -l -F -J`) carry through, toggles (`-t`,`-v`) dropped; first non-flag = target; **any token after target ⇒ None** (one-shot `ssh host cmd` isn't a session). `enter_ssh_file_tree` roots at `.` (remote home), saves pre-ssh root ONCE (nested ssh keeps first). Two clobber-traps the agent fixed: `set_active_workspace_root` early-returns while `file_tree_follows_ssh()` (per-frame cwd sync would repopulate local every frame), and git-status watcher skips when `is_remote()` (root `.` would watch local `./.git`). Enter kicks `populate_from_dir(".")` DIRECTLY, not `populate_file_tree_from_dir` (whose `sync_file_tree_remote_mode` would reset the backend for a non-joined workspace).

**v2 TODO:** writes/save over ssh (`write_file`/`stat` return "not supported yet"; reads work via `request_read_file`→`cat`); finder over ssh (RemoteSearchRoute still daemon-only); mosh (passthrough activates, no tree flip); git badges over ssh; `--color=never ls` assumes GNU ls (a macOS *remote* rejects it); nested ssh keeps first host's tree.

Related: the join/host fixes same session — [[bug_hosted_workspace_badge_and_join_adopt]] `current_workspace_is_remote_joined` now keys on `link_is_peer` (connection-type) not host-identity, so joining a hosted machine shows the remote tree; create-server-in-current-dir shares the current workspace (ShareWorkspace) instead of spawning a duplicate.
