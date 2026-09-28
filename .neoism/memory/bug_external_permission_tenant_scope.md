---
name: "External permission retry and tenant scope"
description: "External permissions and hosted tenant scope"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-23"
updated: "2026-09-23"
---

Correction 2026-08-05: Previous assumption that joined local workspace guests are untrusted hosted tenants was WRONG. User's intended model: Neoism desktop, Neoism server, and local workspace joined over Tailscale are local collaboration; guests have same native agent tool access as host, subject to normal permissions. Only explicitly external hosted control plane (services.hosted=true; Synapse-like deployment) enforces tenant directory isolation and disabled native execution absent isolating provider. Implemented caller::local_collaboration_session(hosted,session): local deployment + local tenant OR workspace ID matching workspace tenant; session_execution_policy(hosted,..) NativeLocal for those including historical Disabled sessions. Updated native process gates, provider tool listing, ToolContext/path authorization, move/deferred move, MCP credential scope, execution_request policy; external hosted remains scoped and native blocked. Integration test executes guest bash and reads/moves external directory on local deployment; caller tests assert hosted workspace name cannot elevate and local guest gets native. Verified interaction_tool_tests 16 passed, caller tests 11 passed, cargo check -p neoism-agent-server and git diff --check pass. `cargo check -p neoism-agent-server -p neoism` fails in concurrent unrelated desktop ingest.rs merge_session_snapshot missing bool argument; do not claim desktop compile verified. Earlier notes claiming joined guests blocked are superseded by this correction.
