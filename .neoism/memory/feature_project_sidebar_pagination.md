---
name: "Per-project sidebar pagination + file-tree icons"
description: "Shipped five-chat per-project paging across desktop/web, independent discovery, native worker cap; file-tree icons/body project labels; all targeted checks pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

The new shared left conversation sidebar discovers projects independently via GET /v2/session-projects -> {items:[{directory,updatedMs}]} instead of grouping a globally limited chat feed. Backend discovers authorized eligible root directories from the compact index, applies tenant/session authorization before aggregation, excludes association-only names, and has a SQL GROUP BY/MAX(updated) fast path for unrestricted unclaimed catalogs.

Project-local pages use /v2/sessions?roots=true&directory=...&exact_directory=true&limit=5. exact_directory applies saved-directory equality before LIMIT, preventing newer hosted-associated nested-project chats from consuming another project's allotment. Existing scoped flat lists retain their broad directory semantics.

Shared paging lives in state/side_panel/project_pagination.rs plus tests. Independent per-project cursor, monotonic tokens, loading/error/retry/queued state; maximum three claims including retired removed-project workers. Initial pages fetch only visible expanded groups (+overscan); Show more fetches that project's next five. Collapse doesn't fetch. Rediscovery preserves history/cursors/inflight claims/selection/viewport. Search-only hydration doesn't inflate browsing allotments; live events promote hits. Blank directory has a pageable Other chats group. toggle_selected_project consumes paging/retry controls; synthetic rows cannot be opened/pinned/renamed/deleted as chats.

Desktop fetch wiring is in agent/api.rs and pane/{session,ingest}; a native three-worker pool enforces the cap even across catalog/endpoint replacement. WASM pumps shared claims each frame. Protocol/daemon uses ListSessionProjects/ListProjectSessions and results/errors carrying catalog identity, generations, directory and cursor. Stale endpoint/root/scope responses are rejected. Web discovery retries are debounced through the shared budget with frame wakes. SSE reconnect refreshes summaries rather than replacing pages with a global head. Global search remains endpoint-authorized.

User visual overrides: sidebar icons now use bundled Nerd Font glyphs, file-tree ICON_FONT_SIZE and draw_icon_centered_with_occlusion; project folders call file_tree::icons::folder_icon and use file-tree chevron sizing. Folder-plus is U+F0257: bundled font does NOT contain U+F65E. Project labels now use the regular body font at FONT_SIZE*s like chat titles, retaining blue color, without P2/bold/extrusion. Default sidebar heading and native/web selector entries say Projects, not All projects. Do not restore old outlined icons/pixel project labels.

Final validation passed: combined native/server/daemon cargo check --tests, final WASM --target wasm32-unknown-unknown --features web --tests, shared cargo check --tests, web typecheck, git diff --check. Tests passed: 9 new shared paging +22 existing project catalog; 6 backend discovery/exact-directory/pagination/OpenAPI; 7 daemon catalog; 4 native URL/stale scope/worker-pool; 18 directly rerun web navigator/dashboard; protocol serialization and additional web transport tests. No release build, app restart, or running-app visual verification.

Editing hazard: fuzzy DrawOpts replacement crossed from header_opts to project label_opts and accidentally erased the renderer middle. Recovered surgically from HEAD's session-row span plus recorded project/paging branches/prologue; preserved dirty navigation prefix/suffix and all recorded original executable snippets/markers (18px dot gutter, Neoism icon/prefix, context/time/hover/marquee/spinner/current-dot styling). Unseen original lines cannot be independently certified byte-for-byte; no lost behavior found. All post-recovery checks/tests above passed. Prefer narrowly contextual exact replacements for repeated DrawOpts blocks; immediately review diff/line counts.
