---
name: "Supabase MCP redirect_uri not allowed — FIXED"
description: "Legacy Supabase DCR client reused after callback route change; missing redirectUri must force re-registration"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-05"
updated: "2026-10-05"
---

# Supabase MCP OAuth `redirect_uri not allowed` — FIXED

## Root cause
A Supabase MCP client dynamically registered before v0.7.70 was allowlisted for Neoism's former callback route, `/mcp/supabase/auth/callback`. Neoism now sends `/v2/plugins/dev.neoism.mcp/supabase/auth/callback`. Older persisted `McpOAuthClientRegistration` records had no `redirectUri`, and `auth_start_with_config` incorrectly assumed a missing URI matched the current default, so it reused the stale Supabase `client_id`. Supabase correctly rejected authorization with `redirect_uri not allowed`.

## Fix
In `neoism-agent/crates/neoism-agent-server/src/mcp_oauth.rs`, dynamic registrations are reused only when their persisted `redirect_uri` exactly matches the requested callback. Legacy records with no URI now trigger one-time dynamic re-registration. Added regression tests for missing and matching redirect URIs.

## Immediate workaround for an older running binary
`neoism mcp logout supabase` removes the complete credential record, including the stale dynamic client registration. Reconnecting then performs dynamic client registration again with the current callback.

## Verification
`rustfmt --check` passed; `cargo test -p neoism-agent-server registration_` passed (4 tests); `cargo check -p neoism-agent-server` passed with unrelated existing warnings.
