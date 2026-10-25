---
name: "Exact inline Git blocks and guarded Restore"
description: "Single-pass canonical source pointer placement, thin hover block controls, exact safe backend mutations, hub-guarded Restore and seeded-replica convergence"
type: "feature"
scope: "project"
origin: "Inline Git mouse and per-edit hover controls implementation"
created: "2026-10-22"
updated: "2026-10-22"
---

Canonical inline Git review now resolves mouse clicks once from the displayed ProjectedCodeRow source line/UTF-8 byte, not a second hit test against stale ordinary geometry after acquiring the canonical CodePane. Pointer placement cancels pending cursor-follow/reveal and pointer scroll; invalid projected gaps must not invent line zero. Tests cover Unicode/tabs/wrap/Vim and stale geometry. Projected caret publishes a full cell for native shape application once, but suppresses any cell overlapping the hover island because TrailCursor cannot clip.

Working review keeps internal hunk anchors but renders thin separator rules instead of @@ text. git_diff_view_blocks.rs derives exact maximal contiguous GitChangeBlock runs (old_start/new_start/old_lines/new_lines), hover-only Stage/Restore islands own input and explicitly occlude source text. Disabled buttons still consume clicks. Untracked Restore disabled; untracked full-addition Stage supported. Do not disable Stage from cached fully-staged status: canonical text can be unsaved. GitDiffPanel::apply_block permits retained canonical sections absent from cached clean status; backend validates actual scope/block.

Daemon git_blocks.rs validates exact displayed block against HEAD/index/worktree, rejects stale/ambiguous/staged-overlap, preserves unrelated staged and worktree edits, literal paths, nested declared-workspace bounds, modes, raw CRLF/EOF, exclusive index lock and alternate index; untracked Stage requires exact whole addition, Restore refuses untracked. Git Restore must use git::handle_with_root_and_crdt: global hub operation gate serializes restore/save/client edits/open/watcher, requires authoritative text and disk equal remembered clean baseline, applies reconciliation and queues Sync/Saved before successful reply. Dirty external reconciliation preserves collaborative edits and reports conflict. Global gate serializes unrelated docs; arbitrary external writers cannot honor it.

Online restore MUST preserve seeded replicas and merge Sync/Snapshot from current state vector. Never drop/reseed and flush old text or publish a disk-derived full replacement: either can reinsert/duplicate restored text. Native attached-daemon local workspaces also require daemon RPC, not pure NativeGitDiffIo; pure block engine only offline. Web save-stage retains exact payload/revision/owner/workspace and refuses replacing an outstanding block. Saved remains unversioned broadcast; concurrent peer save acknowledgment correlation is not globally solved. Offline clean native reload preserves caret/mode/view and transforms positions through reversed block. Markdown restore_scroll_position now clears pending_reveal_line along with caret-follow/momentum so queued outline/roster jumps cannot override restored viewport.

Verification: shared code editor 124, Git diff 92, diff card 20, Chrome events 27, column focus 5, Markdown queued-reveal regression 1; protocol Git 12; backend Git 31; guarded CRDT Restore integration 7; web suite 322 and typecheck/WASM check passed. Native tests typechecked via cargo check, not executed. No live UI/two-machine validation or release build/commit. Earlier full shared suite had unclassified unrelated failures; do not claim globally green.
