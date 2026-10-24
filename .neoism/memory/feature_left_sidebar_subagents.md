---
name: "Left sidebar subagent tree"
description: "Subagents beneath parent in left sidebar; far-right square chase; parent foreground indicator now tracks only its own work, not active children"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

Subagent sessions render as selectable children immediately beneath their parent in the LEFT Conversations sidebar, with curved `╰─` connectors. Removed old right-rail Agents list and its click/key/hand-cursor targets; informational Directory/Session/Usage/Goal/Tasks stay. The square chase remains in the SAME FAR-RIGHT activity slot as master conversation rows, not beside the left connector.

Shared model: `NeoismAgentSessionEntry::{branch_root_id,branch_status,branch_activity_label}`, `NeoismAgentSidePanel::catalog_subagents` family projections injected into the same flattened `sessions` list used by paint, keyboard navigation, hits, selection and scroll. `sync_catalog_subagents_from` handles independent native catalog/pane state; `sync_catalog_subagents` handles single-pane web (drops unsubscribed old families). Exact passive child highlighting via `set_catalog_viewed_session`/`catalog_viewed_session`. Child Active uses the existing white square chase; waiting retains yellow blinking dot, completed green, stopped red. Projection respects lifecycle expiry and retained-viewed historical children. Pruning the raw roster must not rewrite the left catalog cursor when mode=Sessions.

FOLLOW-UP user confirmed visuals work and requested parent no longer spin solely because children are working. `agent-server/src/catalog_activity.rs::activity` now scopes foreground Running to root OWN `execution.session_activities[root].active_segments`, root coordinator run/status/worker/queue. Outstanding children, aggregate family provider segments and child Busy/runs no longer promote parent Running. Root and child can still both spin when both actually work. Permission/background indicators remain family-wide, unchanged. Same canonical four-value wire enum; list/detail/SSE all use this authoritative projection, so no desktop/wasm transport change or renderer heuristics needed. Do not hide parent blindly whenever a child is active. Shared canonical Idle already overrides stale runtime_status=running. Enum docs in core session.rs explain mixed foreground/attention scope.

Native: `desktop/src/screen/bridges/agent.rs` drains/refreshes all live family contexts including hidden tabs/inactive workspaces, projects applicable endpoint/directory families, and retains only roots still owned by live panes (prevents stale endless child spinners after closing last owner). `pane/session.rs::conversation_family_directory` prefers root directory even when viewed child uses a worktree. Left `workspace/sidebar.rs` activation uses branch root for family-tab reuse; pointer on active child toggles root; Enter explicitly selects child. No root pin/delete context menu on child rows.

Web: Chrome captures destination+root atomically on activation and emits through `take_conversation_open_with_family`; WASM bridge exposes structured drain/root ID; TerminalPanel reuses owning family tab/split and preserves unrelated draft input. Runtime/history/cache events project without depending on right-rail rendering. Standalone React Agent GUI is a separate product and was not changed.

Tests: shared sidebar suite, Chrome child navigation, native family routing/ownership, web family-navigation tests; server tests_catalog_activity.rs covers parent-only foreground with root/child concurrency through HTTP and SSE. Files primarily shared panels/agent_pane/state/side_panel.rs + view/side_panel/, desktop bridges/workspace/sidebar.rs + bridges/agent.rs, wasm rendered/catalog.rs, web terminal/TerminalPanel.ts, and server catalog_activity.rs.
