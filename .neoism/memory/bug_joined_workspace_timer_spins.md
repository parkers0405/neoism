---
name: bug_joined_workspace_timer_spins
description: Joined-workspace command block timer spins forever after cmd returns — CLIENT-side; no OSC 133 + blanked prompt defeats the completion fallback
metadata: 
  node_type: memory
  type: project
  originSessionId: 8cdb5853-daa9-46ab-ab00-9a517218826b
  modified: 2026-07-31T21:21:55.353Z
---

Symptom: in a JOINED/hosted workspace (e.g. tailscale piss-desktop), the Warp-style command block duration timer keeps climbing after `ls` already printed and returned. Both joined guests see it. Plain `ssh` in a pane does NOT (it flips to passthrough → no blocks rendered).

WRONG first theory (do not repeat): "remote daemon spawns /bin/sh → no OSC 133." Plain ssh reaches remote shells too, so the remote shell isn't the differentiator. The bug is CLIENT-side in shared+desktop.

Why joined differs from ssh: a joined/adopted pane is a `remote_pty` pane — the adopt path uses `prepared_remote_pty_for_adopt` which is NOT gated on NEOISM_DAEMON_TABS (desktop/src/context/manager/daemon_sessions.rs:261-270), so `ctx.remote_pty.is_some()` is true. `remote_pty_passthrough_target(true)` forces passthrough OFF (desktop/src/host/composer.rs:186), so the joined pane RENDERS command blocks (ssh doesn't). `is_remote_pty` everywhere = `ctx.remote_pty.is_some()`.

Root cause (traced in shared/src/terminal_blocks/input/buffer.rs sync_shell_state:472-518): the block's timer is live until `finished_at` is set (command.rs:46). `finished_at` is set ONLY from OSC 133 C/D/B. The joined shell emits NO OSC 133 at all → `awaiting_command` never latches → `ever_awaited_command` stays false → boot-window composer stays up (that's the visible `SSH >>>`) → none of sync_shell_state's finish conditions ever trigger → spins forever. The desktop fallback `finish_unintegrated_remote_command_at_prompt` IS reachable (remote_pty true, called terminal_compose.rs:226) but dies on `looks_like_shell_prompt("")` because Neoism's OWN daemon block-wrapper BLANKS the prompt (`PROMPT=''`, sessions.rs zsh/bash wrappers). Web/wasm guest had no fallback wired at all.

Fix (client-side, all verified `cargo check` + 59 block tests green):
- shared `finish_unintegrated_remote_command_at_prompt` (buffer.rs): add a BLANK-prompt completion path — if cursor row is empty AND cursor advanced strictly below `output_start_row`, finish. Guarded by a stability anchor (`remote_blank_prompt_anchor` field in input.rs) requiring the blank cursor row to hold the same position across TWO observations, so streaming output (a build printing a blank line mid-stream) never false-finishes. Silent `sleep` (cursor never leaves submission row) also isn't false-finished.
- shared `push_command_block` (buffer.rs): finish any prior Running block on new submit (submitting proves the previous returned) — resolves stacked spinners for zero-output commands too.
- wasm `chrome_bridge_core.rs` (sync_terminal_command_composer_visibility): wire the fallback (cursor row text via `terminal.inner.grid[cursor_line]`) + pass `is_remote_pty=true` (web PTYs are always daemon-backed).

Secondary/defensible hardening from the first (wrong-theory) pass, kept: sessions.rs prefer bash/zsh before $SHELL; ssh_hosts.rs launch remote daemon via login shell; git_branch.rs wasm-gate the git-status producers (this last one was REQUIRED — a pre-existing break blocking the whole wasm build).

Related: [[project_joined_workspace_ssh_model]], [[feature_ssh_follows_tree]], [[bug_terminal_cwd_reroots_workspace]], [[bug_stale_session_attach]]. Not runtime-tested on two machines.
