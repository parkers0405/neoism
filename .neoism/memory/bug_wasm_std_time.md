---
name: wasm-std-time-panic
description: std::time::Instant panics on wasm32 — kitty-graphics code in neoism-terminal-core bricked the whole web app; always use web_time in crates the wasm bundle pulls in
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

2026-06-12: the entire web app froze (no text, endless "recursive use of an
object" errors) because `neoism-terminal-core`'s kitty/sixel graphics code
(commit ede993ec, a different agent) used `std::time::Instant::now()`, which
panics with "time not implemented on this platform" on wasm32. The panic only
fired when graphics escape sequences hit the parser — running codex (which
probes terminal capabilities with kitty queries) poisoned the PTY replay
buffer, so every subsequent page load replayed the sequences and panicked on
boot. Fixed by swapping to `web_time::Instant` (drop-in std re-export on
native) across graphics.rs, ansi/graphics.rs, ansi/kitty_graphics_protocol.rs,
ansi/sixel.rs, handler.rs, crosswords/mod.rs.

**Why:** wasm has no std monotonic clock; `web_time` is the project-wide
convention (shared crate already uses it everywhere).

**How to apply:** in any crate the wasm bundle depends on
(neoism-terminal-core, neoism-ui, neoism-protocol, sugarloaf), never
`std::time::Instant`/`SystemTime::now` — grep for them after merging other
agents' work when the web mysteriously dies. A wasm panic mid-feed manifests
as "frozen app + recursive use of an object" spam, not a visible crash.

2026-06-14 REGRESSION: `ansi/kitty_graphics_protocol.rs:10` came back as
`use std::time::{Duration, Instant}` and bricked web again (identical
"time not implemented" panic from feed_pty_output, then the recursive-use
flood). This file is the recurring offender — its `Instant::now()` call sites
(last_touched, line ~903) use a bare `Instant`, so the import alone decides
std vs web_time. Fix: `use std::time::Duration; use web_time::Instant;`
(handler.rs already does exactly this). Grep `use std::time::Instant` in
neoism-terminal-core as the first check whenever web dies after a rebuild.
