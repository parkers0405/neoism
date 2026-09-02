---
name: project-joined-workspace-ssh-model
description: "Joined workspace = everything resolves to the HOST, ssh-style + 2026-07-28 extensions/toast/icon UI fixes — uncommitted"
metadata:
  node_type: memory
  type: project
---

**2026-07-28, UNCOMMITTED (user tests before commit). Native + Windows `cargo check -p neoism` clean.** Joined workspace = all surfaces resolve to the HOST over the one daemon connection (ssh, not local mirror). Gate: `current_workspace_is_remote_joined()`.

- **Agent tab → host sessions.** Deleted `port+1` resolver; one `agent_reverse_proxy_for_daemon_endpoint` → `<daemon-http>/agent` on both server-switch (`app/mod.rs`, `!is_home` guard) and workspace-switch (`daemon_link.rs`). Host streams SSE through the proxy.
- **LSP → host.** `code/lsp.rs`: `code_lsp_is_remote()`; `code_lsp_target()` None when remote; skip didOpen/didChange + didSave. No local rust-analyzers on host paths.
- **Terminal → host shell.** `daemon_sessions.rs prepared_remote_pty()` bypasses NEOISM_DAEMON_TABS for peer links.
- **Tree + finder search → host disk.** Finder fix: `finder.rs sync_finder_search_route(cwd)` sets remote route at finder-OPEN (was tied to tree populating → guest search hit empty local disk).
- **Notes → host's SINGLE linked vault, scoped.** `WorkspaceSummary.linked_vault_dir`; daemon `resolve_linked_vault_dir`; guest `served_notes_vault_root`; empty-state Create/Select buttons; selector local + "Shared project" separator + one shared vault.

**Nested-folder collapse fix** (`file_tree/update.rs merge_children_preserving_open`): Expand relist preserved open state (Root did, Expand didn't) → deep folders stay open on re-list.

**Extensions fixes** (`neoism-extensions` + `bridges/extensions`): catalog fetch had NO timeout (hung forever = "cannot download") → 15s/60s + hard timeout; rows offered Install when uninstallable → `lsp_missing_status` shows Unavailable + manual hint; missing toolchain → names Node.js/Python3/rustup/Go/Ruby; 45min install watchdog. Source = Mason registry + GitHub releases + pkg managers (NOT Zed's own registry). UI: removed GitHub icon + download count + "…" from cards; extensions buffer-tab uses puzzle `\u{f12e}` (chrome_page branch in buffer_tabs/impl_render.rs).
- **grey-box-off-bar**: indeterminate progress shimmer clipped to CARD not button → detached box sliding outside; fix `intersect_clip([bx,by,bw,bh], clip)`.
- **always-Connecting** (row frozen Installing): pump only runs while in_flight non-empty + only mutates handed pane → failed/abandoned job left row stuck; fix = self-heal in `render_neoism_extensions_panels`: any visible Installing entry not in in_flight → NotInstalled.

**Hamburger** (`chrome_topbar.rs`): removed Workspaces item; icons on all (Settings cog, Web Server globe, Themes brush, Extensions puzzle).

**Sidepanel toggle accent — REAL fix** (`chrome_topbar.rs draw_panel_toggle`): accent pooled in MIDDLE not right pane. Cause: clip window = half the glyph's MEASURED ADVANCE, but codicon ink drawn WIDER than advance → stopped short of right edge. Fix = split at glyph centre, extend accent clip to BUTTON edge (`Right → [center..rect.x+rect.w]`). Don't trust advance-width for icon-ink geometry.

**Toast notifications WRAP** (`notifications.rs`): were single-line truncate-… + hover h-scroll; now greedy word-wrap `wrap_message` (honours \n, hard-splits URLs, cap 10 lines + …), variable card height `toast_height`, render + hit_test wrap identically. Removed scroll_x/scroll_hovered/clamp_scroll/display_message + scroll.rs caller. Long install/LSP errors now readable.

**Honesty note:** LSP installs "failing" (gopls: go required; elixir-ls: click→Cancel→Install) are mostly the NEW honest missing-toolchain messages working — gopls needs Go installed, etc. Agents added timeouts/messages/UNIT tests but did NOT run live end-to-end installs (can't drive GUI+network+toolchains headless).

Agent-page (Alt+A) skeleton loader delegated to a bg agent (reuse side-panel `draw_session_loading_skeleton` fade-in; 1.5s hard-cap so it can't stick). See [[feature-presence-avatars-and-agent-gui]], [[feature-ssh-follows-tree]], [[bug_hosted_workspace_badge_and_join_adopt]].
