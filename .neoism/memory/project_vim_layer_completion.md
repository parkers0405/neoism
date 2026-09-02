---
name: project-vim-layer-completion
description: "Markdown-pane vim engine (shared vim.rs state machine) + palette-as-nvim-hub (?-search, 35 nvim: commands, GoToLine) — landed 2026-07-03"
metadata: 
  node_type: memory
  type: project
  originSessionId: 8c8ab78f-b2bc-4a51-9d08-a3e1f6c67725
---

Two golden-standard layers landed 2026-07-03 (branch tail_boy):

**Markdown/notebook pane vim engine** — `neoism-frontend/shared/src/editor/markdown/vim.rs` (~2700 lines, 52 tests): `VimState::feed` pure resolver implementing `[count1][op][count2](motion|text-object|doubled)` → typed `VimAction`; `MarkdownPane::apply_vim_action` applier goes through the standard undo/CRDT bookkeeping (`replace_range_with` now pub(super)). Covers counts, d/c/y/>/<, D C Y S s x X, w b e W B E ge gE (vim char classes, cw→ce, dw-EOL rule), f F t T ; ,, gg G ^ _ + - { } %, all common text objects incl. ip/ap, r ~ J p P (linewise vs charwise), V linewise visual, n N * # (own minimal search state — palette `/` is a managed-nvim bridge, unrelated), `.` repeat. Desktop glue in `bridges/markdown.rs` `apply_markdown_vim_feed`; register syncs to OS clipboard (linewise = trailing \n). Excluded: Ctrl-V block, insert-text replay in `.`, named registers/marks, H M L, sentence motions. wasm `markdown_key` still has its own mini handler — could swap to pane.vim.feed later.

**Palette as nvim hub** — `?` intercepted like `/` (`OpenSearchPaletteBackward` plan; `rio.search.commit` lua takes backward flag + sets `v:searchforward` so n/N respect direction). 35 `nvim:` catalog entries via `PaletteAction::NvimEx(&'static str)` → existing ex pipeline; `GoToLine` re-enters palette Ex mode; `jumps` added to `rio.command.lua` modal_commands so output shows in the rust modal. Editor-surface gating via `command_visible_for_surface`. Recent-files MRU skipped (needs last-focus timestamps in buffer_tabs/pane_tabs model). Web: NvimEx → PaletteIntent::ExCommand; TS wiring for GoToLine still TODO.

Also fixed pre-existing test bug: `modal_blocking_escape_runs_action_when_available` used hint "o" but `Modal::escape_action` only fires for hint "Esc".

**Gotcha (fixed)**: palette Commands-mode fuzzy matches the query against shortcut hints too — digit-bearing hints (":move +1", ":42") stole typed `:1`/`:42` Enter and ran the matched ACTION (`:1` moved the line down!) instead of the ex jump. Fix: route.rs Enter guard — pure-digit or `$` query dispatches `run_palette_ex_query` before any action match (falls through on terminal surfaces so "0"→Cmd+0 still works). Any future digit-containing shortcut hint is safe because of the guard, but prefer digit-free hints.
