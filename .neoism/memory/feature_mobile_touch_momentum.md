---
name: "Mobile touch momentum + caret-follow suppression"
description: "Shared iOS-like mobile web momentum and touch caret-follow suppression"
type: "feature"
scope: "project"
origin: "current-session"
created: "2026-08-??"
updated: "2026-08-??"
---

Implemented corrected mobile web touch momentum (2026-08): `web/src/services/directTouchScroll.ts` now owns the single recent-velocity sampler + exponential deceleration policy (110ms sample window, 0.32s tau, 72px/s firm floor, 6000px/s cap, dominant-axis lock). `TerminalPanel.ts` records direct 1:1 touch motion and replays release frames through the same direct routing for tabs, chrome panels, agent, code, Markdown, terminal; new touch cancels momentum and suppresses its click. Shared Rust touch APIs remain non-inertial and now report bounds where possible so host momentum stops at edges. Code `scroll_touch_pixels` no longer uses wheel drag-along: caret stays fixed and `follow_cursor=false`; Markdown does the same. Explicit navigation/edit paths already rearm follow. Added TS acceleration/decay/stop/axis/bounds/no-click tests and Rust code/Markdown suppression/rearm tests. Checks passed: npm typecheck, directTouchScroll node tests 7/7, cargo fmt --check, cargo check neoism-ui+neoism-terminal-wasm, wasm32 cargo check --features web, cargo test touch_scroll 7/7. Full npm suite attempt was 83/86 because checked-in wasm touchPolicy behavior was stale versus concurrently edited expectations (vertical promotion/long-press), unrelated to momentum; no release build or wasm-pack generation run.
