---
name: bug_hosted_workspace_badge_and_join_adopt
description: "Hosted-workspace network badge (FIXED), guest join adopt silent-fallback (OPEN, needs runtime log)"
metadata: 
  node_type: memory
  type: project
  originSessionId: a955c5e2-195e-4bec-be36-a54386f2ee00
  modified: 2026-07-28T21:42:12.714Z
---

Three top-right server-UI issues reported 2026-07-22. Two shipped, one diagnosed-not-fixed.

**#1 network badge on hosted workspace tab — FIXED.** The badge pipeline already
existed end-to-end (`workspace_icon_kind_for_index` in
`desktop/.../daemon_sessions.rs` reads `WorkspaceSummary.visibility` → `"shared"`
→ glyph in `shared/src/widgets/island/render.rs`), but nothing ever set
visibility. Fix: `declare_startup_workspace` (`neoism-workspace-daemon/src/workspace/manager.rs`)
now flips the `--workspace DIR` workspace to `WorkspaceVisibility::Shared` right
after create — that dir EXISTS to be broadcast, so it's shared the moment the
daemon binds. Also gave the network glyphs their own colors in island/render.rs
(`shared`/`tailscale` = green BROADCAST_COLOR, `joined`/sandbox = blue JOINED_COLOR)
so a live-on-network tab reads at a glance. Manual palette "Share workspace"
(`PaletteAction::ShareCurrentWorkspace`) still works for non-hosted dirs.

**#2 UPDATE 2026-07-28 — FIXED + whole joined-workspace editing wave.** Guest join opening a new tab: root cause was `open_or_adopt_daemon_workspace` (open_create.rs) — its first "already in a grid?" check false-matched the current LOCAL grid and took the no-op switch branch; fixed by requiring `current_adopted_workspace_id == this id` before short-circuiting, else fall through to adopt. Plus `workspace_owned_locally` (daemon_link.rs) defaulted unknown ids to `true` (owned) — wrong for a guest; now `.unwrap_or(!link_is_peer)`. Added branch logging in open_or_adopt + all 3 adopt bail-outs. THEN the editing bugs on a joined workspace: (a) **code files showed nothing / flickered** — `drain_code_pane_crdt` (code_crdt.rs) bailed on `error.is_some()`, but a joined pane's LOCAL read always errors (file's on host) → never bound to CRDT. Fix = add `if code.remote_content_pending { return false; }` gate mirroring `drain_markdown_pane_crdt` (markdown's comment literally describes the flicker: "content flashes, goes blank, tab reads dirty"); added `CodePane::mark_remote_loading`/`apply_remote_source` + `request_remote_code_content` in open_path_in_code (daemon ReadFile → FileContent seed). (b) **ssh/joined code os-error-2** — same static-read seed. (c) **tree folders snapping shut** — per-frame cwd→root sync re-rooted a joined workspace from the guest's local cwd every frame; guarded `set_active_workspace_root` with `!force && current_workspace_is_remote_joined()` (mirrors the ssh `file_tree_follows_ssh` guard). DESIGN note: same abspath on same daemon ⇒ same `file://<path>` CRDT doc, so host opening a dir locally + guest joined = intentional collaboration (the blank-until-touched was the seed bug, now fixed). See [[feature-ssh-follows-tree]].

**Multiplayer session lifecycle (2026-07-28, same wave):** (rehost) hosted daemon from `create_and_join_local_server` (palette.rs) wasn't detached → died with app on SIGHUP; now `setsid` (unix) / `DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP` (win). `SavedServer.hosted: Option<HostedServerSpec{state_dir,workspace_dir,port,require_auth}>` persisted in servers.json (serde default → old files load); Screen→App bridge via `hosted-spec.json` sidecar in state dir. `drain_server_switch_results` Err arm relaunches the daemon (thread_local relaunch-once guard) then re-dials. (kick) new `WorkspaceServerMessage::HostEnded{reason}`; daemon `broadcast_host_ended` on shutdown_signal (150ms flush) + StopSharingWorkspace; socket.rs `host_ended_rx` select arm delivers it; client ingest `handle_host_ended` (guards link_is_peer, `detach_adopted_grid_sessions` = no ClosePty, stashes `daemon.cache.host_ended_reason`); app drain (after go_home) → Warn notification + `mark_host_ended`(→Offline) + switch home (drops guest reconnect loop). Both targets compile, 17 tests pass.

**#2 [ORIGINAL DIAGNOSIS, superseded above] guest joins, "connected" shows but host workspace NOT added to guest strip.**
Diagnosed to a funnel, not pinned. Connected-dot (`Online`) and workspace-adopt are
set by DIFFERENT things in `complete_server_switch` (`desktop/src/app/mod.rs`):
dot flips purely on socket-up; adopt needs the tree + `needs_initial_workspace_adopt`.
When the tree lands (`apply_daemon_workspace_message`), `adopt_daemon_workspace`
(`daemon_sessions.rs`) can return `false` → `open_or_adopt_daemon_workspace`
(`screen/panes/open_create.rs`) SILENTLY falls back to pointer-only
`switch_daemon_host_workspace` = "connected but no tab". Three bail-outs: no prepared
remote PTY (RULED OUT — runtime IS wired via `attach_daemon_client_with_runtime`),
capacity reached (only this one logs, at warn), root context creation fail.
Already-fixed red herring: guest used to re-publish its own workspaces into the host
tree and win the adopt — now guarded by `if link_is_home` in
`attach_daemon_client_with_runtime` (`daemon_link.rs`). NEXT STEP: have the guest join
with `RUST_LOG=neoism::workspaces=info,neoism_frontend=debug`; open_create.rs peer-join
log + daemon_sessions.rs bail-out lines name the branch. Also worth: the fallback is
silent — a failed join looks identical to a success. Relates to
[[project_shared_notes_agent_scoping]], [[feature_per_window_servers]],
[[project_workspace_root_model]].

**#3 agent composer Up/Down over soft-wrapped lines — FIXED.** Three defects, all in
shared composer (`panels/agent_pane/`): (a) movement walked rows by CHAR INDEX while
the renderer wraps/places caret by proportional PIXEL width — now `InputWrapRow`
carries per-boundary x offsets (`input_controller.rs`), movement snaps to nearest-x
gap; (b) no goal column — added sticky `goal_x` (mirrors terminal composer's
`desired_visual_column`), cleared on edits/horizontal moves, carried across pane
rebuild via `with_goal_x`; (c) box shows only MAX_INPUT_LINES(5) rows and always drew
the LAST 5 — now the visible window follows the caret row
(`view/user_input.rs`). Renamed `set_input_wrap_ranges`/`current_input_wrap_ranges`
→ `_rows`, storing `Vec<InputWrapRow>` in both shared (`state.rs`, `state/caches.rs`)
and desktop (`pane.rs`, `pane/render_state.rs`) pane state. Gotcha: the
`neoism_ui_impl_agent_user_input!` macro must FULLY-QUALIFY
`$crate::panels::agent_pane::input_controller::InputWrapRow` (expands at desktop call
site with no import). 15 input_controller tests pass. Relates to
[[feature_agent_input_bar]], [[feature_bottom_composer]].

Verify note: `cargo build --bin neoism` green after fixing 2 PRE-EXISTING WIP errors
that were unrelated to this work but blocked the build (untracked
`editor/code/multicursor.rs` E0502 — split `lines[sl].len()` into a local before the
mutable borrow; `command_palette/actions.rs` non-exhaustive match on user's new
ToggleWordWrap/ReplaceInFile → Editor-surface, ProjectProblems → always-visible like
ToggleGitDiffPanel). Those three are `CommandService::Code` (handlers:
toggle_code_word_wrap / open_finder_buffer_replace / open_project_problems in
desktop palette.rs) — surface choices are a judgment call, adjust if intent differs.
