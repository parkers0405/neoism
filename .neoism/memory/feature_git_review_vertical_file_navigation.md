---
name: "Git review vertical file navigation"
description: "Alt+Up/Down jumps focused diff file headers; explanation of source-caret vs pixel scrolling and context expansion"
type: "feature"
scope: "project"
origin: "User Git review keyboard navigation follow-up"
created: "2026-10-21"
updated: "2026-10-21"
---

Added Alt+Up/Down file-header navigation while the Git review body owns keyboard focus, native + shared/web. Shared GitDiffView::navigate_file(next) in panels/git_diff_view_focus.rs walks this view's own sections, expands target, clamps at ends, handles empty/missing selection, and preserves selected diff column. Clears edit_focused/reveal_edit_on_focus and queued edit_request so previous canonical caret cannot snap back; retains edit_path/document_path for live dirty-buffer painting without reseeding or losing undo. Existing [/] aliases delegate to the same API. Native intercept is before handle_buffer_tab_focus_key in selection/key_event.rs; consumes release but navigates only on press, excludes Island/tabs/sidebar/palette/finder/modal/menu focus. Shared chrome/git_diff_focus.rs early Alt-only helper excludes competing focus/overlays and never focuses tabs. Historical views navigate their own sections, never request editable working documents. Also restored native left diff focus when Alt+Left cannot leave the leftmost split and no left sidebar exists. Normal-arrow/wheel distinction is intentional, NOT an explicit hunk navigation binding: wheel moves pixel viewport with no caret changes; editable Up/Down move canonical full-source caret, then GitDiffView::reveal_editor_cursor follows; sync_live_editor/build_review_diff includes 3 lines of context + caret, so traversing omitted unchanged source changes excerpts/hunk heights and can look jumpy. Read-only Up/Down scroll three displayed row heights. New regressions cover file edges, folds, missing/empty selection, canonical caret no-snap-back, history isolation, both columns, other focus owners/inactive tabs, modifier/release policy.
