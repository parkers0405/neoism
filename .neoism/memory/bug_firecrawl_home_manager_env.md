---
name: "Firecrawl MCP Home Manager environment"
description: "Home Manager GUI environment caused Firecrawl MCP keyless requests"
type: "bug"
scope: "project"
origin: "project"
created: "2026-07-16"
updated: "2026-07-16"
---

Neoism local desktop embeds neoism-agent-server on 127.0.0.1:4096 in the desktop PID. MCP `{env:VAR}` expansion happens there. On the user's Home Manager + Hyprland setup, `FIRECRAWL_API_KEY` is declared in `home.sessionVariables` and generated at `~/.nix-profile/etc/profile.d/hm-session-vars.sh`, but `/usr/bin/bash -l -i -c env` does not source that script. Therefore GUI credential hydration missed the key and Firecrawl saw keyless requests despite `mcp.json` containing `Bearer {env:FIRECRAWL_API_KEY}`. Fix added in `neoism-frontend/desktop/src/agent_server.rs`: source Home Manager session vars first and accept them only if a provider credential exists, then fall back to login-shell probing. Live workaround resolved the header into `~/.config/neoism/mcp.json`, chmod 0600, disconnected/reconnected Firecrawl via local `/mcp/firecrawl/{disconnect,connect}`. Also imported FIRECRAWL_API_KEY into Hyprland and systemd user environment for future launches. Verified real local `/mcp/firecrawl/tools/firecrawl_search` succeeds without rate limiting. Synapse Docker agent was stopped at user request; mistaken Docker env wiring was reverted.
