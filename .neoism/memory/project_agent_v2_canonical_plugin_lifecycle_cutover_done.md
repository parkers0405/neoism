---
name: "Agent V2 canonical plugin lifecycle cutover — DONE"
description: "Canonical plugin lifecycle now includes capability attenuation, closed-state invalidation, retry-safe shutdown, deterministic retirement, route ownership, metadata preservation, and API v2"
type: "project"
scope: "project"
origin: "adversarial lifecycle audit remediation"
created: "2026-08-26"
updated: "2026-08-01"
---

# Agent V2 canonical plugin lifecycle cutover

Completed and adversarial-audit hardened on branch `neoism_agent_v2` in the main workspace (uncommitted).

Canonical cutover:
- Removed legacy production plugin trait/registrar paths; zero-reference architecture guard remains.
- Async `PluginFactory -> PluginInstance` install is end-to-end.
- `PluginContributions` carries declarations, all runtime sources/tools/hooks, HTTP/WebSocket routes, and typed services.
- Host install filters disabled plugins before graph/create, orders dependencies, validates scope/capabilities, starts/readiness-checks, rolls back reverse, and provides reverse shutdown.
- Built-ins and declarative/process plugins all enter canonical lifecycle.

Audit hardening:
- Every first-party definition declares explicit required host capabilities; declarative process requirements derive from process/network config. Production grants are explicit and factory contexts are attenuated to declared capabilities; missing capabilities reject before create.
- Registry snapshots have an active/closed token. Shutdown invalidates before cleanup. Route dispatch, hook/tool execution, and generation resource access reject closed generations.
- Workspace lifecycle has lock-synchronized closed state; state access requires installed manifest and active generation, preventing disabled or post-shutdown lazy allocation. Workflow state is allocated only when installed/enabled.
- `InstalledPlugins` and `ManagedPluginInstance` use cancellation-safe retryable shutdown state machines. Failure/cancellation resets attempt state but does not reactivate generation; resource entries are removed only after successful inner/resource shutdown.
- Generation leases are explicitly counted/notified. Published replacement launches slot-owned retirement tasks that wait for all leases, retry shutdown until success, and can be deterministically drained; best-effort generation Drop cleanup was removed.
- Managed factories cache descriptors.
- Host rejects impossible plugin/route scope combinations; active dispatch checks closed state and verifies session ownership against canonical workspace.
- RegistrySnapshot now preserves typed service contribution metadata. Testkit host-stamps metadata and validates declarations, WebSockets, tools, sources, hooks, services, and routes.
- `PLUGIN_API_VERSION` intentionally bumped from 1.0.0 to 2.0.0; discovery contracts consume the constant dynamically.

Tests added/restored cover capability denial/attenuation, shutdown error retry, cancellation retry, closed snapshots, disabled/closed allocation, managed resource preservation, deterministic retirement and lease draining, descriptor caching, route scope rejection, WebSocket/hook/declaration conformance, first-party service/source/tool/route conformance, and MCP post-shutdown invalidation.

Verification: warning-denied checks for core/plugin-api/builtins/server including tests; core 17 passed; plugin-api 22 passed; builtins 70 passed; server 409 passed/5 ignored. One parallel full-suite LSP restart timeout was rerun in isolation and passed, then the complete server suite passed. Whole-workspace rustfmt remains pre-existing broadly red and was not applied. Unrelated frontend and Firecrawl files remained untouched.
