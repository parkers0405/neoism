---
name: "App-wide workspace cd palette"
description: "App-wide cwd palette and terminal-local prompt implementation, review fixes, verification"
type: "feature"
scope: "project"
origin: "session implementation"
created: "2026-09-05"
updated: "2026-09-05"
---

Implemented app-wide workspace directory palette across desktop/web/shared. Alt+D, splash Change Directory, status cwd click all open the same palette without creating/focusing a terminal. Palette target captures workspace id/root; stale submission after workspace switch is rejected. App-level cd/OSC 777 re-roots workspace; ordinary shell cd stays terminal-local. Status cwd is workspace root; command composer gets focused terminal's cwd, including split panes and daemon SessionCwd metadata, rendered `cwd >>>` / `SSH · cwd >>>`. Dynamic directory recommendations, ghost completion, Tab/Shift+Tab, recents, current/parent/home/workspace rows; WASM searches keyed by workspace+root+query to reject stale replies. Windows drive/UNC path detection, parent and joining added. Desktop cd - uses palette per-workspace root history; daemon has previous_workspace_roots for web. Web waits for HostWorkspaceUpserted before continuing palette and displays daemon Error. Mobile status touch suppresses compatibility mouse event without preventing initial touchstart. Obsolete terminalDirectory TS helper/tests deleted. Verification: cargo check neoism-ui/terminal-wasm/neoism/workspace-daemon; cargo test command_palette 93 pass; status cwd test pass; daemon reroot test pass; cargo test -p neoism --no-run; npm typecheck; npm test 169 pass; git diff --check. cargo fmt --all --check is not clean because the dirty worktree contains broad existing unformatted changes; do not bulk-format/rewrite concurrent edits. Unrelated buffer_tabs impl_core.rs/tests.rs preserved. No commit/release.
