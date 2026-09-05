---
name: "Mobile keyboard flash, overlay fields, and Enter follow — FIXED"
description: "Deferred iOS editor/overlay focus, exact overlay text-field classifier, and post-scroll repeated Enter caret-follow repair"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-09-01"
updated: "2026-09-01"
---

Implemented mobile web keyboard/caret fixes in the heavily-modified main worktree. iOS root cause: code/Markdown used synchronous touchstart provisional focus; even though viewport insets were suppressed and blur happened at scroll promotion, WebKit had already started visual keyboard presentation. `mobileEditingPolicy.ts` now reserves touchstart provisional focus only for the Agent composer and defers terminal/code/Markdown/overlay fields to trusted touchend. `TerminalPanel.ts` applies the resolved tap against pre-keyboard geometry, then commits/focuses, so reflow cannot retarget it; one-finger touchstart remains unprevented. Modal rows no longer generically request keyboard.

Central overlay field contract: wasm `mobile_text_field_at` returns exact family for command palette (including themes/mashups), finder/search, Settings search/text/dropdown/keybind, file-browser path/search, universal modal, Agent question free-text, and Extensions searches. Shared panels own exact hit rect tests; TS `mobileTextFieldIntent` makes overlays an ownership boundary so misses never fall through to the Agent composer. Agent regular picker/secret search remains composer-owned; permissions with buttons are correctly non-text. Overlay list drags defer focus and cannot flash keyboard. Chrome byte routing blurs after an overlay closes and Settings key capture now accepts soft-keyboard Text events.

Caret root cause: Markdown draw consumed `follow_cursor` against the previous frame's stale `cursor_rect` before the virtual line splice was applied; then anchor restoration forced follow false. `scroll_cursor_into_view` now leaves follow armed while `pending_line_edit` exists, and virtual anchor restoration preserves an explicit edit's follow request. Code/Markdown expose explicit rearm helpers; wasm direct key routes rearm on typing/Enter/backspace/navigation, including boundary no-ops, while touch scroll alone suspends follow. Direct mobile byte decoding is centralized/tested for repeated Enter and nav.

Verification: web typecheck + all 138 tests pass; cargo check neoism-ui passes; cargo check neoism-terminal-wasm --target wasm32-unknown-unknown --features web passes; focused newline tests 2/2; field-hit tests 2/2; full neoism-ui lib 2301/2301; git diff --check passes. Global cargo fmt --check is blocked by unrelated pre-existing formatting in concurrent file-browser/desktop/protocol work; scoped touched modules pass rustfmt except editor_panes.rs, whose reported diffs are unrelated pre-existing safe-area formatting.
