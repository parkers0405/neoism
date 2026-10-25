---
name: "Project chat navigator + P2 panels"
description: "Final chat navigator handoff: P2, modal search, same-workspace reroot, no counts/dates; edits frozen for another model"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

Branch nightly-experimental-project-chats; checkpoint 3b8bfa3b7. Feature changes uncommitted, nothing pushed. User handed sidebar to another model: ALL sidebar layout edits and subagents are finished/stopped. Do not continue changing its layout without a new request.

CURRENT UX: original Press Start 2P section headings/default body font (Geist experiment reverted). Chats title; plain New chat row with outlined compose icon matching screenshot; thin separator under New chat; Search row opens global title/message modal; project dropdown and folder-plus. Folder chat counts removed. Exact-directory sidebar is flat, with no date/Pinned headers, pins remain sorted first. Home/filesystem-root shows chats grouped by directory. Gutter50->18. Scope independent of activechat executiondir. All-project search stays within authorized connected endpoint; no aggregation of unrelated remote DBs.

LATEST USER OVERRIDE: project selection MUST re-root SAME CURRENT WORKSPACE through normal directory-change route, NOT create/adopt/switch workspace grids. Native conversation_workspaces.rs uses existing persisted registry off-thread, commit_workspace_directory shared with existing Workspace directory picker. Local set_active_workspace_root(root,true); joined set_daemon_workspace_root(current_id,root) retains ownership guards. Folder-plus registers root then commits it; All projects restores remembered home/root PATH per host connection. No injected PTY cd. New stale-first-shell-cwd observation guard prevents an unchanged oldcwd undoing explicit reroot, while genuine subsequent cd still follows normally.

New shared widgets/conversation_navigator.rs renders centered Search / anchored Projects using inline_picker. Current project initially selected; explicit keyboard selection preserved across row refresh. Native screen/bridges/conversation_navigator.rs owns debounced singleflight worker and full metadata cache; stale query/endpoint replies rejected. sidebar.rs open_conversation_catalog_entry(entry,originserver) uses actual saved directory and scope+endpoint guard rather than old raw-directory mismatch warning. Context pin/rename/delete scopes corrected too. Header enabled setter idempotent (don't erase hitrect/dropdownanchor everyframe). Native host paint enables header and redraw_reason owns active navigator.

Web: authenticated SearchThreads/ThreadSearchResult + ListProjectRoots protocol; current-root callback -> SetWorkspaceRoot, not grids. Authorized FilesLocations home cached per client/auth/catalog identity, cleared on identity change; arbitrary initial project is NOT a dashboard fallback. WASM/Chrome dashboard-root setter makes advertised home produce catalog_directory=None through actual set_workspace_root. All projects disabled if no confirmed/observed root. Final activation fix: Chrome keeps Activate navigator query live until browser validates it, browser then closes; Dismiss closes immediately. Closing before validation erased query and prevented nonempty search results opening.

Verification: native/daemon/WASM --tests cargo checks passed before final dashboard/activation refinements; 21 sidebar tests, 5 widget tests, 11 native conversation tests, canonical scoped-SSE test, and daemon persisted registry test passed. Latest web typecheck and37 directly rerun navigator/dashboard/protocol tests passed (subagent broader57 passed). Last sharedChrome-test +WASM-check command timed out at120s with no result; do not claim latest full WASM rerun passed. Four shared-agent baseline failures previously verified unchanged at checkpoint (detailchild click, two mobiletakeover dismissals, tab agentchip transition); no release build/app restart/visual verification. Canonical SSE test fixtures need id,sequence,source,schemaVersion,timestamp,type,data—not legacy properties-only.
