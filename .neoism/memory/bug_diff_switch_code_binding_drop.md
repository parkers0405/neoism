---
name: "Code document binding loss on diff source switch — fixed"
description: "FIXED browser source-switch doc binding drop: bindings park with CodePanes, null focus flushes, parked CRDT inbound routes preserve dirty/undo"
type: "bug"
scope: "project"
origin: "editable Git review integration safety fix"
created: "2026-10-21"
updated: "2026-10-21"
---

FIXED browser editable-diff document loss on A->B->A. A review click changes desired edit path before CodePane is switched. JS's activeCodeBufferId becomes null while pane A still exists; the old code_crdt_pump(None) dropped its binding without flushing, and activation dropped code_doc_binding again. Reopening could re-seed stale text and reset dirty baseline/undo. Chrome now parks/restores CodeDocBinding with CodePane via parked_code_doc_bindings in file-open and diff activation; None focus flushes pending local edits/history but RETAINS binding. Inbound editor_crdt_apply resolves matching active OR parked document by buffer ID. Existing retained bindings prevent stale file reads from reseeding/resetting document state. Explicit editor/workspace disposal clears maps. Current diff document remains canonical even when header/sidebar has keyboard focus: Stage uses GitDiffView.document_path, not focus-gated editing_path, to identify the current source for save-before-stage. 6 genuine Rust tests cover seeded A/B switching, unsent edits, remote parked updates, replica identity/sticky anchors, dirty state and undo/redo; 10 WASM host tests and actual wasm32 --features web check pass. Full shared suite has 14 other failures (details being audited separately), so don't report a globally green suite. Root files: shared/chrome.rs, shared/chrome/content.rs, wasm/rendered/editor_panes.rs. Parent feature architecture in feature_git_inline_review_editor.md.
