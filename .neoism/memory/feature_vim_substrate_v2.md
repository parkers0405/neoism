---
name: "vim-substrate-v2"
description: "Vim substrate v2 landed: block visual, named registers, marks, jumplist, macros, redo"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-07-30"
updated: "2026-07-30"
---

# Vim substrate v2

Landed on the shared resolver + code/markdown appliers + desktop host wiring.

## Added
- Visual block via Ctrl-V (`EnterVisual { blockwise: true }`). Markdown treats block as charwise visual for now; code pane tracks `vim.visual_block`.
- Named/special registers (`"a`, `"0`-`"9`, `+`/`*`, blackhole `_`) via `VimRegisters` on `VimState`.
- Marks: `ma`, `'a`, `` `a ``
- Jumplist: Ctrl-O / Ctrl-I; large motions push jumps in code applier
- Macros: `q{reg}` / `q` stop / `@{reg}` / `@@`
- Redo: Ctrl-R through `feed_ctrl` + `VimAction::Redo`
- `VimApplied.sync_clipboard` so named-only writes don't clobber OS clipboard
- `VimApplied.replay_keys` for macro playback

## Key files
- `neoism-frontend/shared/src/editor/markdown/vim/model.rs` — actions/stages/registers/marks/jumps/macros/feed_ctrl
- `neoism-frontend/shared/src/editor/code/vim.rs` — code applier
- `neoism-frontend/shared/src/editor/markdown/vim/pane.rs` — markdown applier + register commit on yank/delete
- `neoism-frontend/desktop/src/screen/bridges/code/input.rs` — Ctrl-V/R/O/I + macro replay
- `neoism-frontend/desktop/src/screen/bridges/markdown/input.rs` + `bridge_policy.rs` — markdown Ctrl chords

## Verification
- `cargo check -p neoism-ui` / `cargo check -p neoism` clean
- `cargo test -p neoism-ui --lib vim_` → 45 passed

## Still open vs Neovim
- True blockwise edit ops (I/A/r across columns) incomplete
- Numbered delete register rotation edge cases
- Insert-mode Ctrl-R register insert
- Changelist (`g;`/`g,`)
- Full Ex engine, folds, incremental treesitter, extension API
