---
name: "Workspace picker registry 404 toast fixed"
description: "Expected registry GET404 when management disabled; quiet capability handling, complete authorized history union, no false registry persistence"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

The Projects picker union now loads registered workspaces (when supported), ALL authorized chat-bearing directories from /v2/session-projects.items (not recent-chat pagination), and the declared current root. Native previously read only the management registry, so historical project directories were missing. Native uses off-thread fetching and a pending-gated 15-second TTL while open; web NavigatorProjectDiscovery starts fresh on every open, routes replies through accept(), merges source results, and cleans up on dismissal/endpoint/client/root/workspace replacement. Registry labels win over inferred basename labels; directory-key identities deduplicate and preserve selection. Root-only Add workspace is a sentinel intercepted before directory activation and routed through the validated browser hook; it must never become a directory/workspace path.

User then saw toast `Workspace registry: Neoism Agent HTTP 404 Not Found` while project choices loaded. Cause verified: /v2/management/workspaces is the CORRECT route, and daemon proxy preserves it, but agent mounts management routes only when NEOISM_AGENT_MANAGEMENT_API=1 (default disabled). This is an optional capability, not a project-discovery failure. Native now classifies actual parsed HTTP status BEFORE converting to strings; ONLY this optional listing GET's HTTP404 returns source-unavailable (Result<Option<Value>,String>, None). No recurring warning toast. Registration POST404, auth401/403, 5xx, malformed JSON, and network failures remain actionable. Do not swallow arbitrary strings containing 404, and do not enable the management API by default just to suppress this warning.

With management disabled, native can enumerate known authorized chat-history directories and the current root; it cannot claim to list every registered history-free workspace. Root Add workspace may still perform validated same-workspace rehome without pretending a registry entry persisted. Web uses the authenticated daemon registry; only explicitly typed registry HTTP404 is quiet. Wrong-endpoint/summary HTTP404 and arbitrary error strings remain reported. No filesystem/home guessing or limited recent-chat fallback.

Files include desktop agent/api.rs, bridges/conversation_workspaces.rs; web conversationNavigatorHost.ts/tests and actual TerminalPanel.ts integration. Validation: optional registry API2 tests, native bridge15 tests, web targeted/adjacent57 tests; parent reran37 navigator/dashboard-root tests and web typecheck. Native cargo check and scoped whitespace passed. All jobs collected; no release builds. Later sidebar keyboard/search/Recent UI changes are separate work; do not infer current layout from this bug note.
