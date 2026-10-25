---
name: "Editable Git review native lifecycle safety"
description: "Native review close promotes dirty hidden sources without replacing buffers; real-tab fallback and actual-owner dirty guards prevent silent loss"
type: "feature"
scope: "project"
origin: "final editable review lifecycle audit"
created: "2026-10-21"
updated: "2026-10-21"
---

Final safety fix for native editable Git reviews: canonical CodePanes are ordinary hidden stacked contexts so CRDT/document IO keeps owning them without opening a source tab on each diff click. On review CLOSE, any dirty hidden source owner is promoted into ordinary BufferTab metadata BEFORE removing the review route; canonical buffer/text/undo/CRDT is NOT replaced. Clean cached-code fallback selected after close is registered as a real source tab, never displayed under Terminal metadata. Both review-close entrypoints and close-split use this promotion policy; unsafe cross-workspace promotion refuses close visibly. Review modified dots now derive actual canonical document dirtiness independently of keyboard focus; historical commit tabs remain clean. Destructive window/app/workspace close, server-buffer guards, and automatic root terminal teardown check actual CodePane owners including hidden ones, refusing dirty close with save warning. Explicit :qall! workspace discard remains supported. Tests added for promotion/fallback, canonical text+undo preservation, multi-document idempotent registration and route dirty flags; native --bin neoism --tests cargo check passes, tests typechecked notrun. Existing targeted shared Git58, code119, diff_card14, scroll74, history2, Chrome events27, daemonGit14, browser tests/typecheck and wasm checks passed across integration. Broader shared run had Markdown/agent/terminal failures with unverified baseline; tree-width failure was a stale fixture corrected to use Chrome.set_file_tree_width before install. No release build/commit or interactive/two-machine validation. See feature_git_inline_review_editor.md and bug_diff_switch_code_binding_drop.md.
