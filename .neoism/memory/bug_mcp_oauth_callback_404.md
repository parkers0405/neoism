---
name: "MCP OAuth callback HTTP 404 — FIXED"
description: "MCP OAuth generated a stale legacy callback path, causing localhost HTTP 404; fixed and released in v0.7.70"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-30"
updated: "2026-08-30"
---

## Root cause
MCP OAuth's default redirect URI still generated the removed legacy route `/mcp/{name}/auth/callback`, while the plugin-first router exposes only `GET /v2/plugins/dev.neoism.mcp/{name}/auth/callback`. Providers redirected successfully to the agent server but received HTTP 404.

## Fix
Changed `neoism-agent-server/src/mcp_oauth.rs::redirect_uri` to generate the plugin route and updated `default_oauth_redirect_uses_the_active_agent_server` regression coverage. Focused server test passes. Released in v0.7.70, commit 260dd03771359ba356bf54cfbfc390d96c2619b2.

## In-flight recovery
A callback already sent to the stale path can be recovered by replacing `/mcp/<name>/auth/callback` with `/v2/plugins/dev.neoism.mcp/<name>/auth/callback` while preserving the `code` and `state` query string.
