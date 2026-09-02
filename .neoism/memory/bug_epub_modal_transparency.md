---
name: "EPUB modal transparency"
description: "EPUB annotation modal opacity fixed with Sugarloaf late-overlay special case"
type: "bug"
scope: "project"
origin: "User screenshot and implementation on 2026-07-26"
created: "2026-07-26"
updated: "2026-07-26"
---

## Symptom

The EPUB annotation modal (`Note on highlighted text`) appeared transparent: EPUB body glyphs rendered through the modal body and controls even though modal rectangles had opaque colors.

## Root cause

EPUB body glyphs flush after Sugarloaf's normal shape pass. The shared modal was called later in Rust render order but still submitted to the normal pass, so deferred EPUB glyphs punched through its background.

## Fix

In desktop `host/run.rs`, capture whether the active grid item is EPUB before later mutable grid work. When a modal is active over EPUB only, wrap `self.modal.render(...)` with `sugarloaf.set_late_overlay_mode(true/false)`.

This is intentionally modal-bounds-only. It does not add a fullscreen scrim, suppress the whole EPUB text layer, or affect modals over other pane types. Modal rectangles and modal text share the late overlay pass and therefore compose correctly above EPUB glyphs.

## Verification

- `cargo check -p neoism --message-format=short` passed.
- `cargo test -p neoism-ui epub_note_modal_uses_markdown_pane_and_serializes_newlines --lib` passed.
- `cargo fmt --all -- --check` and `git diff --check` passed.
