---
name: "EPUB multiline notes + collections"
description: "Multiline EPUB annotation modal and multi-collection Markdown projection architecture"
type: "feature"
scope: "project"
origin: "session correction after user feedback"
created: "2026-07-21"
updated: "2026-07-21"
---

Correction to earlier implementation: syntax-colored wrapped ModalInputSpec was rejected because it was not the real Markdown renderer and Shift+Enter was unreliable. EPUB add/edit note modals now instantiate and own an actual shared editor::markdown::MarkdownPane. All input methods delegate to MarkdownPane (insert_text/newline/backspace/delete/navigation), modal submission serializes pane.lines, and UniversalModal calls editor::markdown::render::render for the editor region.

Keyboard contract is editor-native: Enter inserts a Markdown newline; Ctrl+Enter/Cmd+Enter submits/saves. This avoids relying on Shift modifier timing and permits ordinary long-form editing. Focused test widgets::modal::tests::epub_note_modal_uses_markdown_pane_and_serializes_newlines verifies real pane ownership, newline persistence, and action serialization. Cargo check neoism-ui+neoism and EPUB collection integration test pass.
