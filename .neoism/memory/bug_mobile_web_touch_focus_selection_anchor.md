---
name: "Mobile web touch focus/selection/viewport — FIXED"
description: "Mobile web provisional focus, touch selection drag, momentum sampling, and viewport anchoring"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-09-??"
updated: "2026-09-??"
---

Implemented deep mobile-web gesture correction. `TerminalPanel.ts` now treats text focus as provisional intent: editor/Markdown/agent composer focus synchronously on touchstart for iOS, but `MobileKeyboard` suppresses provisional visualViewport insets and blurs on scroll promotion; terminal capture is deferred until a resolved input tap. Direct touch routing cancels focus and momentum at bounds. `directTouchScroll.ts` normalizes epoch/monotonic event timestamps, supports sorted historical batches, ignores stationary touchend velocity samples, allows a 180ms sparse-final grace, and keeps firm launch/stop floors.

Shared `touch_policy.rs` long press is now a persistent drag state (`SelectWord` -> `ExtendWordSelection` -> `EndWordSelection`) rather than one-shot. WASM routing supports terminal, code, Markdown/notebook, and agent selection. Both original word edges are retained so crossing the word anchors the opposite edge; Unicode word/grapheme stops come from `editor/text_selection.rs`; edge auto-scroll continues from the long-press timer and release preserves selection.

Viewport anchoring: code `CodeBuffer` records structural cursor-row displacement from full/window undo metadata (line snapshots now include total line_count); `CodePane` rebases scroll/current/raw target before reveal. Remote deltas also set the row delta. Markdown virtual line insert/delete always captures/restores the virtual scroll anchor even for explicit caret edits and suppresses recenter afterward; manual touch/momentum still leaves follow off.

Regressions added in TS/shared Rust for terminal deferred tap vs scroll focus, provisional commit/cancel, sparse/stationary/coalesced momentum, timestamp normalization, touch promotion, long-hold extension/release, bidirectional + Unicode anchored selection, code deletion anchor, and Markdown structural anchor policy.

Checks passed: npm typecheck; npm test 90 passed/2 skipped only because checked-in generated wasm predates new held-word actions; cargo check neoism-ui; cargo check neoism-terminal-wasm --target wasm32-unknown-unknown --features web; cargo check neoism desktop (pre-existing warnings); cargo test neoism-ui --lib 2240/2240; cargo fmt --all -- --check; git diff --check. No release build and no generated wasm rebuild.
