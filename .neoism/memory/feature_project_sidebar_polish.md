---
name: "Sidebar project picker and Pinned/Recent polish"
description: "Improved project switcher; root-only folder-plus; explicit Pinned/Recent; body-font Show more; Projects divider; checks pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

User's latest left-sidebar refinements are implemented after per-project paging. Project dropdown now has Switch project heading, regular Find a project... search input, keyboard hints, six visible rows, anchored 320-420px width clamped to viewport. Disabled projects render muted normal two-line items retaining path/footer/current marker, not section captions; selection and activation skip them. New inline_picker::render_limited_with_disabled adds presentation support without changing existing row/view structs/APIs. Dashboard row is still named Projects (user explicitly removed All projects), with Dashboard footer. Native/web registry rows deduplicate lexical directory paths and sort current first then alphabetical; authorization and same-workspace reroot behavior unchanged. Explicit selection and top visible project identity survive registry refresh; registry errors notify instead of bloating dashboard footer. Native and shared/web dropdown pointer motion now selects enabled hovered rows without activating; stale hit rects invalidated on data changes. Native motion path app/window_event/mouse.rs and shared Chrome navigator both route the shared pointer_move method.

Folder-plus/AddWorkspace appears ONLY in root Projects dashboard (catalog_directory None), not scoped project view. Renderer reclaims reserved button width in scoped view; state header_action_enabled gates registration and hit lookup, scope transitions remove stale AddWorkspace hits, Chrome rejects queued AddWorkspace after leaving root. Native dropdown Add workspace row/activation also root-only. Existing authorized dashboard/root detection is unchanged; do not replace with guessed home paths.

Browsing now explicitly emits Pinned above Recent within each expanded project (or directly in a scoped project view). Only nonempty sections render; pinned chats are not repeated in Recent. Section captions have no directory metadata (otherwise is_project would misclassify them), are nonselectable, and are indented by depth. Search results omit browsing section captions. Pin/unpin via live upserts preserves selected session, viewport anchor, and per-project cursor; Show more remains the last row of each project. This supersedes earlier memory saying scoped catalogs have no Pinned/Recent headers.

Show more/Loading/Retry labels now share project_label_opts with folder labels: regular body FONT_SIZE*scale, blue accent, no P2/font override/bold/extrusion; the same options are used for truncation and painting. Added a thin divider below Projects dropdown row, matching the existing New chat divider (muted 0.22 alpha, same inset and thickness).

Files: shared widgets/conversation_navigator.rs + inline_picker.rs; sidebar state/side_panel.rs + project_catalog_tests.rs + project_pagination/tests.rs; view/side_panel/draw.rs; shared chrome/navigator.rs; native bridges/conversation_navigator.rs + conversation_workspaces.rs and app/window_event/mouse.rs; wasm rendered/conversation_navigator.rs; web conversationNavigatorHost.ts/tests. Preserve existing dirty workspace changes.

Validation passed: 46 project-filtered shared tests; full side_panel filter 105 tests; Chrome navigator and shared dropdown suites; native navigator/workspace9 and web navigator11; web typecheck; final native cargo check -p neoism --tests and final wasm32 cargo check --features web --tests; git diff --check. Tests include pin/unpin viewport preservation, root/scoped stale AddWorkspace hits/actions, matching folder/page typography, disabled items and pointer hover/stale hit invalidation. No release build, application restart, or running-app visual check.
