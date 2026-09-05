---
name: "Web Vim Ex commands quit app — FIXED"
description: "Web Vim Ex q accidentally selected PaletteAction::Quit; fixed with real Ex mode, shared plan protocol, dirty/save-ack BufferTabs lifecycle"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-08-01"
updated: "2026-08-01"
---

Fixed 2026-08-xx. Root cause: WASM code `:` and markdown `open_palette` used `command_palette.set_enabled(true)`, leaving the shared palette in ordinary commands mode. Query `q` then selected `PaletteAction::Quit` before the Ex hint, and TerminalPanel dispatched terminal/PTY shutdown. Fix: both WASM entry points call `enter_ex_mode`; PaletteIntent::ExCommand carries a `plan` classified by shared Rust `parse_ex_command` + MarkdownExCommandPlan/GlobalExCommandPlan. TS `vimExHostPolicy` enforces q dirty refusal, q! force, w save-only, wq/x save then close only after matching CRDT Saved or host FileWritten ack, qall dirty refusal/qall! drain active-workspace tabs while retaining root terminal/browser page. BufferTabs replay now carries modified state; markdown_dirty bridge parallels editor_dirty. Ex close uses BufferTabs and collapses an owning focused split; no Ex path invokes app/window quit. Plain q remains in shared Vim key handling because only colon enters Ex mode. Checks: web tsc + 98 tests; cargo check neoism-ui + wasm native; wasm32 cargo check; focused shared palette test. cargo fmt --check remains globally blocked by unrelated pre-existing format diffs.
