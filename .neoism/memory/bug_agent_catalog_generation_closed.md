---
name: "Agent catalog generation 410 fixed"
description: "Fresh Agent catalogs returned 410 because built-in capability admission failed and errors became closed snapshots; fixed grants, tenant-aware fallible routing, local/joined/hosted tests."
type: "bug"
scope: "project"
origin: "agent"
created: "2026-09-01"
updated: "2026-09-01"
---

# Agent catalog 410 from failed plugin generation construction — FIXED

## Symptom
Fresh debug servers returned HTTP 410 `plugin.generation_closed: The plugin generation is shut down` for `/v2/agents` and `/v2/providers/configured`; Agents and Models pickers could not load. Joined/hosted routing also risked selecting the local runtime.

## Root cause
`production_workspace_grants()` omitted `ProcessSpawn` and `SecretRead`, while trusted built-ins (artifacts, providers, MCP, LSP, VCS, PTY and others) declared them. Initial registration and safe-default fallback both failed before publication. `AppState::plugin_snapshot()` swallowed construction errors and returned `closed_snapshot()`, which route dispatch mislabeled as a retired generation. `plugin_route_dispatch()` also ignored authenticated `CallerClaims.tenant_id` and acquired the local runtime.

## Fix
Restore `ProcessSpawn` and `SecretRead` to workspace descriptor admission; concrete external broker calls still require installed brokers. Add fallible tenant-aware snapshot acquisition, route plugin dispatch through authenticated tenant identity, keep non-local tenant generations isolated from local ambient scopes, use tenant-aware acquisition in the Agent tool registry, and map construction failures to internal/503 `plugin.generation_unavailable` rather than 410. Desktop model catalog requests now include the workspace directory like Agent catalog/config requests.

## Verification
Targeted tests `first_catalog_requests_publish_an_active_workspace_generation` and `joined_and_hosted_catalogs_use_distinct_active_tenant_generations` pass. `cargo check -p neoism-agent-server` and `cargo check -p neoism` pass with pre-existing warnings.
