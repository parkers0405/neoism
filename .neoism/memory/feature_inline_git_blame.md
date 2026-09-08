---
name: "Zed-style current-line inline Git blame"
description: "Current-line trailing avatar + author/relative age, NOT a gutter; editor.git-blame defaults off; daemon HEAD cache, shared dirty mapping/render, desktop+wasm, hover details; checks passed"
type: "feature"
scope: "project"
origin: "implementation + explicit user correction"
created: "2026-09-08"
updated: "2026-09-08"
---

# Current-line inline Git blame

User clarification supersedes the initial full-gutter request: **NO blame column or width/wrap changes**. Show only a trailing annotation on the focused cursor line. Implemented in main workspace without commits.

## Verified upstream Zed behavior
- `crates/git_ui/src/blame_ui.rs::render_inline_blame_entry`: FileGit icon + `Author, relative time`; summary hidden by default.
- `crates/editor/src/element.rs::layout_inline_blame`: padding after visual line end.
- `crates/editor/src/git.rs::render_git_blame_inline`: focused editor/newest selection head, nonempty line; NOT hover-only. Timer resets on cursor movement.
- `assets/settings/default.json`: inline delay 0ms, padding 7 em widths, show_commit_summary false; hover_popover_delay 300ms.
- GitHub provider builds CDN `/u/e?email=<encoded>&s=128`; missing/bot email uses commit API author.avatar_url, optional GITHUB_TOKEN.

## Implementation
- Typed **`editor.git-blame`**, default false; GUI Editor > Inline Git blame; palette **Toggle Inline Git Blame** (per-pane override, reset by config reload).
- Native config watcher applies to all panes and renderer default. Web polls GetConfig every 15s (no config watch verb), applies changed snapshots, and refreshes immediately after GUI persistence. Disabling clears attribution/pending replies and stops new requests; existing bounded requests can finish and are ignored.
- Shared `editor/code/blame.rs`: scoped request state, stale-ID rejection, HEAD-to-dirty-buffer unchanged-line map, decoded thumbnails. `gitdiff.rs` adds bounded Myers correspondence (128 edit depth); known changed/new lines Uncommitted; over-budget unknown spans Attribution unavailable rather than guessed authors.
- Shared `editor/code/render/blame.rs` plus `render.rs`: current visual row only, 7-cell gap, actual circular GitHub image or generic Git icon (never fabricated identity), muted author + relative time; clipping/ellipsis, no wrap/scroll/geometry reservation. Blank lines hidden. Existing trailing diagnostics take priority.
- Hover details after 300ms: author/email, summary, full SHA, UTC date. Popup planned before code paint for text occlusion; native/shared LSP hover excludes blame hit rectangles. Only focused pane paints; frame-local image overlays cleared on both hosts. Image IDs collision-checked against raw pixels.
- Protocol `GitClientMessage::Blame { path }` -> `GitServerMessage::Blame { snapshot }`. Snapshot has baseline, per-line commit indices, commit metadata and deduplicated base64 32x32 RGBA images.
- Daemon `git_blame.rs`: read-only libgit2, resolved HEAD + canonical repository + repo-relative path cache. Discovers nested repo from confined parent directory; handles unborn/untracked files; rejects escaping symlink parents and nonregular/binary blobs. Socket dispatch is async over push lane, never blocks PTY/CRDT traffic. Disconnect cancels HTTP; separate permit remains held inside uncancellable blocking Git work.
- Desktop `screen/code_blame.rs`: owning daemon endpoint/connection/workspace/path scope, reply routing via `git_state.rs`, active-pane pump via code LSP, low-rate HEAD refresh and timeout wakes. Client-local documents on joined hosts excluded (no path/identity leakage).
- Wasm editor_panes exposes request/reply pump, TerminalPanel sends scoped Git requests through ProtocolClient. Shared renderer/state, no dependency on agent UI.

## Bounds / limitations
512 KiB and 20,000 lines; max 4,096 commits / 1 MiB metadata. 16 HEAD snapshots, 10-minute cache TTL. At most 32 distinct avatar identities per snapshot; four HTTP workers, 4-second per-request timeout, 6-second whole-avatar phase budget, 512 KiB response cap, constrained image decoder; cache 256 avatar lookups including negative results. GitHub-only avatars, generic Git icon offline/unavailable. No popup action buttons or commit navigation. HEAD refresh roughly 15s. Source-layout fidelity inspected, GUI not visually run.

## Validation
Passed cargo check -p neoism-backend -p neoism-protocol -p neoism-workspace-daemon -p neoism-ui -p neoism --tests; cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown; final cargo check -p neoism-workspace-daemon --tests after nested-repo guard; web tsc --noEmit; git diff --check. Added regressions for HEAD-only/read-only author identity, unborn/untracked/binary files, nested repo/path confinement, dirty/repeated-line mapping, bounded fallback, config-off/stale replies, portable dates and typed grouped config. Tests were COMPILE-CHECKED, not executed, respecting cargo-check-only workflow. No release build or commits.
