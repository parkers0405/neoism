---
name: shared-side-panels-hosting
description: Shared Chrome now hosts the rich GitDiffPanel (Alt+G) + NotesSidebar (Alt+N); web feeds them via refresh-intent → daemon fetch → host_set push
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

As of 2026-06-11, shared `Chrome` (neoism-frontend/shared/src/chrome.rs) owns
`git_diff_panel: GitDiffPanel` (rich right column from panels/git_diff) and
`notes_sidebar: NotesSidebar`. `set_layout` carves their widths (notes left of
content, git panel right inset — chrome reflow per [[feedback-chrome-panel-style]]).

Web data-flow pattern (panels have no IO on wasm):
- toggle (Alt+G / Alt+N, chrome key shortcut or TS hotkey) queues
  `pending_git_panel_refresh` / `pending_notes_refresh`
- TS pumps `takeGitPanelRefresh()` / `takeNotesRefresh()` in
  `drainChromeIntents`, fetches via `client.requestGit` (Status + Diff{path:null},
  group hunks per path; hunk `patch` includes the `@@` header so shared
  `parse_diff_into` parses it) or recursive `requestFiles(ListDir)` on
  `<root>/notes`, pushes back via `git_panel_set_files/set_diff`,
  `notes_set_entries`
- row activations land in `drain_panel_open_paths()` → same open pipeline as
  file-tree picks (`openActivatedPaths` in TerminalPanel.ts)

**Why:** desktop hosts these panels in its own renderer; web parity needed
Chrome-side hosting + daemon-backed data.

**How to apply:** new shared side panels should copy this
toggle→refresh-flag→host-push→drain shape. The notes "workspace" on the daemon
is just `<root>/notes` (InitNeoismWorkspace action scaffolds it), much simpler
than desktop's vault config. Daemon `git_changes_snapshot` now returns LINE
totals (numstat + untracked line counts) to match desktop's status pill.
