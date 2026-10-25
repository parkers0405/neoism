---
name: "Inline MCP authorization and accurate connection status"
description: "User-confirmed inline MCP auth, accurate runtime readiness, debug callback routing, connected Done dismisses picker; checks pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

User live-tested inline MCP auth successfully in debug. Earlier loader stopped at `MCP client runtime is not connected yet` because /auth/authenticate returns that synthetic Failed once credentials exist. Desktop authorized() now accepts ONLY that exact readiness sentinel, then POST /connect; only JSONtrue becomes Connected. Other failures remain errors. Generic runtime status must not become Connected from credentials alone.

MCP authorization is an extension of existing McpActions picker above composer, not a modal. Shared McpAuthorizationDetail/name/body/status/busy/url, set_mcp_authorization/clear_mcp_authorization and saved normal actions preserve server context through catalog refresh. Wrapped readonly header uses exact sidebar square loader and actual painted row/header hit geometry. Busy detail keeps loading=false and Open authorization page/Back interactive; refresh cannot erase detail or open a global error modal. Typing/paste/backspace cannot invisibly filter readonly actions. Completed Connected state uses **Done / oauth_done**, closing the entire picker without touching input draft or connection; pending/failed states retain Back. Stale Done cannot dismiss an unfinished attempt. User explicitly requested Done rather than navigating backward after success.

Desktop PendingMcpOAuth owns atomic cancellation identity (existing ModalActivity type, not modal UI). Pane Drop/newattempt/scope change cancel worker; scope change closes old detail. Back/Escape restores normal actions without stopping watcher; picker dismissal also leaves bounded watcher running. Progress never reopens hidden UI; reopening actions reattaches only busy attempts. Retry connection skips auth and starts Connecting. Consumed authorization URLs are cleared before Connecting/after runtime failure. No MCP global modal UiEvents remain.

Debug routing: desktop api.rs::mcp_oauth_redirect_uri derives callback from actual pane HTTP(S) endpoint with custom port/workspace proxy path, sends redirectUri only to /auth; poll/connect directory-only. http_authority keeps nondefault Host ports in HTTP/SSE. Explicit configured OAuth redirect still wins. Backend is_peer_callback_uri validates consistently encoded names (form_escape spaces '+' converted to '%20'), including UTF8/slash/percent. Same-store alternate processes can share saved credentials; isolated mismatch normally stays needs_auth or fails state lookup, unlike readiness sentinel. Never log OAuth secrets/query. Final desktop/shared --tests, backend --tests, wasm32 cargo checks and git diff --check PASSED including Done. Tests compile-checked, not executed; real-provider success confirmed by user rather than agent UI automation.
