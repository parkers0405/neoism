---
name: "Agent plugin openness architecture"
description: "Broad Agent plugin openness: package trust, scoped generations, brokered capabilities, Lua runner, lifecycle and verification"
type: "project"
scope: "project"
origin: "agent implementation session"
created: "2026-08-01"
updated: "2026-08-01"
---

Implemented broad Agent plugin openness across plugin-api/server/SDK/runtime packaging. Process-v2 now supports exact owner identity (package revision, registry generation, scope/workspace/scope ID), generation-bound bounded streams, unary Agent/Command/Skill/SystemContext/Prompt/Config services, providers/routes/WebSockets/MCP/message parts, reverse host capability brokerage, owner-aware cancellation, audit metadata, dynamic lease revocation, and distinct Network/ProcessSpawn/TaskSpawn/SecretUse/SecretRead/PromptRead/MessageRead/ResponseTransform/ProviderAccess/PolicyInvoke capabilities. SecretUse remains opaque and does not imply raw read.

Added JSON-only shared neoism-plugin.json discovery with contained entrypoints, reverse-DNS IDs, whole-package sha256 revisions, file/byte budgets, symlink rejection, exact external trust pins, installation/workspace provenance, requested/granted capabilities, and global/user/workspace/session scope checks. Workspace config cannot self-authorize. Shared packages use strict candidate startup and preserve last-known-good generations; legacy process plugins preserve degraded startup behavior.

Added isolated separately packaged neoism-agent-lua-runner using mlua only in the subprocess crate, with stripped ambient Lua libraries, contained require, bounded frames/queue/memory/instructions/time, exact-owner reverse host calls, serialized callbacks, logical cancellation, all process-v2 contribution kinds, and sibling-executable resolution. Included in Linux/macOS/Windows build workflows and WiX MSI.

Added ScopedPluginRuntimeRegistry with independent exact-key PluginHost generations, candidate-first publication, rollback, lease revocation-before-cleanup, exact session cleanup, global/installation-user activation during workspace reconcile, session activation during create, and shutdown cleanup. Global/user scoped snapshots are composed as lower-priority ordinary contributions into workspace request snapshots (workspace registrations win conflicts); scoped active snapshots also feed lifecycle state. Exact session contributions are activated and lifecycle-managed but request selection remains session-context-specific future plumbing where existing call sites expose only workspace identity.

Added typed lifecycle DTOs/states and /v2/plugins/lifecycle, plus agentLifecycleInfo in /v2/plugins. States include discovered, permission-required, trusted, enabled, loading, active, degraded, failed, disabled, update-available, restoring with source/revision/scope/scope ID/requested+granted capabilities/retained revision/lease/actionable diagnostics.

Verification passed: cargo check for plugin-api/server/lua-runner; package tests 5; scoped runtime rollback test; process host tests 14; process wire tests 6; API lifecycle test; Lua runner tests 4; TypeScript build+plugin-host conformance; scoped git diff --check. Architecture guards now pass internal_plugins AppState guard after narrowing scoped grants to an EventPublisher; 3 unrelated concurrent/pre-existing failures remain: pty_if_allocated lease, v2_routes concrete IDs 8>6, plugins/subagents server refs 43>42. No release build or TASKS checkbox update.
