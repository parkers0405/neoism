---
name: "Notes rename/delete macOS latency"
description: "Alt+N Notes rename/delete bypass synchronous project Git/tree refresh; shared mutations are vault-scoped"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-14"
updated: "2026-08-14"
---

## Root cause
Desktop Alt+N Notes rename/delete reused generic file-tree modal actions. A successful local mutation called `refresh_file_tree_entries()`, which synchronously rebuilt the project tree and ran `git rev-parse` plus `git status --porcelain=v1 -z --untracked-files=all` on the UI thread. Modal dispatch then recursively refreshed the Notes vault, and the native watcher could refresh it again after 200 ms. macOS process launch/APFS/iCloud costs amplified this.

## Fix
Modal file mutation actions now carry a `notes` origin flag. Notes keyboard/context menu actions set it; project-tree actions do not. Local Notes rename/delete bypass project-tree/Git refresh and refresh only the Notes sidebar. Rename also rebinds the open Markdown pane and buffer tab. Shared Notes rename/delete use files-plane operations scoped to `served_notes_vault_root()` and correlated `pending_remote_notes_mutations`, then relist Notes rather than the project tree.

## Key files
- `neoism-frontend/shared/src/widgets/modal.rs`
- `neoism-frontend/desktop/src/screen/bridges/file_tree/create_rename.rs`
- `neoism-frontend/desktop/src/screen/bridges/file_tree/path_ops.rs`
- `neoism-frontend/desktop/src/screen/bridges/file_tree/daemon_sync.rs`
- `neoism-frontend/desktop/src/screen/lifecycle/modal.rs`

## Verification
`cargo check -p neoism -p neoism-ui` passes. Exact changed files pass `rustfmt --check`; repository-wide fmt has unrelated pre-existing diffs.
