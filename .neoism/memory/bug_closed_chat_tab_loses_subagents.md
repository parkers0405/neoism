---
name: "Closed chat tab loses running subagents - fixed"
description: "Fixed closed chat tabs losing sidebar subagents: independent server family SSE, scoped reconnects, 126 tests passed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-18"
updated: "2026-10-18"
---

Closing the last desktop chat tab used to stop its family SSE observer and agent.rs then pruned sidebar children using live_catalog_families derived exclusively from surviving panes. Switching to another tab worked because hidden panes were retained. Sidebar families now come from the independent server catalog stream, not copied chat panes. This supersedes the live-pane retention design in feature_left_sidebar_subagents.md; do not restore tab-owned pruning or keep stale copied running rows.

Core CatalogFamilySnapshot/CatalogBranchSnapshot and SESSION_CATALOG_FAMILY (session.catalog.family) carry a complete per-root replacement: owning directory, execution ID, family revision, branch lifecycle, title/agent/depth/permission. Server catalog_activity.rs reads fresh atomic current-execution runtime snapshots on relevant edges and baseline, authorizes the owning root (including cross-directory worktree children), indexes session metadata once rather than rebuilding an archive-sized map per root, and emits deterministic tree preorder. Root CatalogActivity stays separate: running children must not force the root foreground indicator to Running.

Shared SidePanel.apply_catalog_family_snapshot owns per-root activities, execution retirement, and terminal revision watermarks. Lower revisions cannot resurrect children; equal revisions update permissions/titles without restarting completion grace; higher lifecycle revisions can reopen Task continuations within an execution. Historical terminal baselines remain hidden; observed completions expire through renderer ticks without any open chat. Legacy sync_catalog_subagents / retain_catalog_subagent_families APIs and desktop/Chrome/WASM pane-copy callers were removed. Selection and viewport anchoring remain in set_catalog_subagent_rows.

Desktop AgentSessionCatalogUpdate::Family applies through catalog-only ingest; reconnect clears stale families before baseline. Protocol CatalogFamily carries the JSON snapshot to the browser. CatalogReset { directory: Option<String> } precedes daemon reconnect baselines; browser clear_catalog_families_in_scope clears only that directory because multiple scoped streams coexist. ThreadDeleted is session-unscoped and removes unopened roots/children. Endpoint/directory invalidation and deletion clear family metadata.

Verified 126 targeted tests: 91 shared catalog tests, 21 daemon agent::events::tests, 2 desktop left_catalog tests, and 12 server catalog_activity_tests. Desktop cargo check -p neoism --bin neoism and wasm32 browser --features web --tests compilation passed; git diff --check passed. Desktop pane tests live in the binary: cargo test -p neoism --bin neoism left_catalog (NOT --lib, which matches zero). Server tests_catalog_activity.rs is included as catalog_activity_tests: cargo test -p neoism-agent-server --lib catalog_activity_tests (NOT the filename, which matches zero). SSE fixtures must use canonical envelopes through SseDecoder, not legacy properties-only JSON.

Same task removed only the separator immediately below New Chat / above Search in shared view/side_panel/draw.rs, retaining the separator below the directory selector. App was not launched/restarted and no release build was run.
