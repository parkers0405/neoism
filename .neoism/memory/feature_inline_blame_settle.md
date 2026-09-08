---
name: "Inline blame: 250ms cursor settle, scroll preserves annotation"
description: "User-approved 250ms shared cursor-state debounce; scroll keeps annotation but dismisses hover; cached avatars retained, requests gated; desktop/CLI-launch/web/joined; checks passed"
type: "feature"
scope: "project"
origin: "User-approved refinement"
created: "2026-09-08"
updated: "2026-09-08"
---

# Inline blame cursor-settle refinement

Supersedes the immediate-display timing in [[feature_inline_git_blame]]. User approved **250ms debounce** after cursor activity; this intentionally differs from Zed's default 0ms. Keep the inline annotation (NOT a width-reserving gutter).

Implementation is shared across desktop (whether launched via terminal CLI or app launcher), web and joined graphical code editor panes. Terminal-only agent CLI has no graphical inline blame. Existing exclusion for client-local documents on a joined remote host remains unchanged.

- `shared/src/editor/code/blame.rs`: `observe_cursor` tracks (source line, byte column, document revision, explicit placement revision), with shared web_time::Instant deadline. `update` observes before request eligibility, `needs_request` defers while settling (preserving deadline through scope reset). Cursor movement/edit clears hover, tooltip and annotation hitbox, retaining snapshot/images. Pending requests may finish/cache; no new requests until settled.
- `CodeBuffer::cursor_placement_revision` initialized in buffer.rs, bumped in input.rs::set_cursor_position even for a repeated click on the same cell. Other movements/vim/held arrows observed by actual cursor coordinates, not key identity. No idle reset from CRDT pumps found.
- `render.rs`: observes cursor and viewport; hides inline annotation only during cursor settle, not due to scroll. Viewport change dismisses popup without restarting timer. Popup stays dismissed under stationary pointer until real pointer motion re-arms hover. Existing natural clipping still applies.
- `CodePane` scroll_pixels, scroll_touch_pixels, set_scroll_progress dismiss hover immediately (including boundary gestures). They never touch cursor settle state.
- Redraw uses existing finite shared render animation result: `blame_settling && focused` => desktop `frame_ctx.has_animation` and wasm `Chrome.animations_active` => TerminalPanel.scheduleDraw. Stops after 250ms; no permanent redraw or raw epoch-f32 timer.
- No config or launch-path changes. User's /home/parkersettle/.config/neoism/config.json already has editor.git-blame=true and was not modified.

Validation: passed `cargo check -p neoism-ui -p neoism --tests`, `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown`, `git diff --check`. Added deterministic clock-injected regression tests for 249/250ms boundary, unchanged updates, column motions, repeated same-cell placements, edits, scroll-only popup dismissal, hover re-arm, scope reset request gating and image retention. Tests compile-checked only, no executions/release builds/commits. GUI timing not runtime-tested.
