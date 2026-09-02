---
name: project_cross_laptop_status
description: "Work-anywhere status after Wave 6 (2026-06-09) — real cross-host promote with git transfer, pairing plane, tailnet drag targets, bidirectional CRDT"
metadata: 
  node_type: memory
  type: project
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

Goal: HOST > WORKSPACE (Ctrl+Shift+W) > TABS; throw a workspace to another laptop over Tailscale; web controls a desktop workspace. Home model = hybrid per-workspace (`running_on_host_id`), ≥1 always-on hosted node recommended.

**Status (Wave 6 merged on `work_anywhere`, 2026-06-09):**
- Waves 1–5 done: HostWorkspaceTree authority, daemon auto-mints token + tailnet-bind launcher, `--ssh-host` attach, shared live PTY proven, snapshot/session-transfer primitives, CRDT seeding, split parity, drag→promote/demote UI.
- **Promote is REAL, not metadata-only** (6B, merge `1c366ae0`): source `/workspace/promote` = auth → resolve target (paired-host name → URL → tailnet peer) → git push (409 no-remote/detached) → WorkspaceSnapshot (tracked diff + untracked) → target `/workspace/receive` clones/reuses, replays snapshot, remaps tab cwds, registers same workspace id in its tree → source flips `running_on_host_id` + drops local state only after 2xx. Desktop drag client unchanged (serde aliases preserve old wire shape).
- **Pairing plane** (`neoism-workspace-daemon/src/hosts.rs`): target `POST /pair` → code; source `POST /hosts/pair` claims + persists `paired_hosts.json` (0600); `GET /hosts`. No manual NEOISM_HOST_URL.
- **Tailnet peers as palette drag targets** (6A: `daemon_client/tailnet_peers.rs`).
- **Bidirectional CRDT** (6C): nvim→doc incremental (`nvim_buf_attach` on_lines → minimal UTF-16 replace), doc→nvim atomic set_lines; echo guards = lua `neoism_crdt_applying` flag + origin-client-id skip. `tests/crdt_bidirectional.rs` 7/7 incl. live embedded-nvim round trip.
- Proven by `tests/two_daemon_promote.rs` (two in-process daemons, full move incl. uncommitted diff + tab remap).

**Daemon unification (2026-06-10, commit 00996f4a):** the standalone `neoism-workspace-daemon` now ALSO binds the desktop's default unix socket (`$XDG_RUNTIME_DIR/neoism.sock`, probe-before-unlink, `--no-unix-socket` to disable) — a plain `neoism` launch attaches to it instead of spawning an embedded daemon, so desktop+web share one workspace tree. Web Alt+W = create workspace on connected host (desktop Ctrl+Shift+W `create_tab` parity); picker is palette "Workspaces", live-refreshed via `refresh_workspaces_palette`. Wasm gotcha: any TS-called export that opens/changes a chrome modal must call `chrome.set_layout(viewport)` after — panels only draw inside their layout rect (palette sub-modals were invisible until the next input event reflowed).

**Wave 8 cutover (2026-06-10, commits after 00996f4a):** 8A `PtySession::remote` (neoism-terminal-pty/remote.rs + session enum) — desktop pane renders a daemon shell through the SAME Machine/Messenger channels; manager bridges PtyOutput→feed, ops→link; env-gated `NEOISM_DAEMON_TABS=1`; `kill(0)` self-HUP guard in Context::drop (remote shell_pid=0). 8C adopt: Workspaces pick on foreign workspace builds a real Island grid attached to EXISTING sessions (`register_adopted_context`: bind + map + resize-nudge, NO CreatePty); `adopted_workspaces` map (grid root route→daemon ws id) keeps daemon identity in publish/pick/echo paths. Both code-complete, NOT live-exercised. Remaining 8x: geometry policy, editor-tab adopt + layout_snapshot, web outbound CRDT, workspace-identity unification, two-machine, cloud. Vault map: ~/Neoism/Vaults/Neoism/TASKS.md Wave 8.

**Still open:** real two-physical-machine validation (hosted git remote, TLS/wss hop, live tailnet resolution, operator pairing round-trip); live PTY processes don't travel by design (tabs re-created); control-loop input forwarding (`ControlWorkspace`) still metadata-only.

**Landmines:** daemon in-crate `#[cfg(test)]` module (workspace.rs) pre-existing-broken (8 errors, stale struct shapes) — use `tests/` integration style; 3 `nvim::tests::*_when_available` lib tests fail on clean trees; agent worktrees can branch from stale bases — ALWAYS check `git merge-base` against the work branch before trusting/merging agent output. See [[bug_connectinfo_unix_daemon]], [[reference_warp_oz_architecture]], [[reference_codex_remote_architecture]].

**Wave 7 multiplayer — COMPLETE (2026-06-09, merged through 74d5b5ac):** 7A ephemeral presence plane (no-echo, TTL, `NEOISM_PRESENCE_TTL_MS`); 7B markdown pane↔CRDT (`doc_sync.rs` shadow-diff binding, caret transform); 7C remote caret rendering (`remote_carets.rs`, exact under Live Preview); 7D per-user undo (Yrs UndoManager, local-origin tracked, 500ms capture groups); 7E soak gate (`tests/crdt_soak.rs`, 4 clients × 220 edits, strictly linear broadcasts, `NEOISM_SOAK_SEED`); 7F web parity (TS presence store/publisher, web markdown is read-only DOM — presence yes, co-edit no = the remaining parity gap); 7G roster + `NEOISM_DISPLAY_NAME`/config display-name override + click-to-jump. Engine is **Yrs not Loro** (memory elsewhere says Loro — wrong).
