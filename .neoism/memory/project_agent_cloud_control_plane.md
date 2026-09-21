---
name: "Agent cloud control plane"
description: "Cloud multi-tenant Agent control plane implementation status, security decisions, and remaining work"
type: "project"
scope: "project"
origin: "active implementation"
created: "2026-08-01"
updated: "2026-08-01"
---

# Neoism Agent cloud control plane

Updated 2026-08-?? in active implementation.

Implemented host-injected TenantResolver, tenant/actor/execution policies, explicit TenantQueryScope, tenant-scoped search/pagination/semantic lookup, tenant-scoped MCP credentials and permission approvals, hosted native-process denial, remote-only MCP for hosted, plugin invocation tenant context, and execution contracts.

Core persistence now normalizes tenant_id on sessions, messages, prompt_queue, interaction_requests, events, and message_embeddings with migration version 6/backfill from session_list_index or local. Durable event replay accepts explicit tenant scope. Session creation persists tenant, creator, execution policy, and tenant quotas.

ExecutionProvider now has required provider selection. A local-native provider preserves local command execution and rejects hosted provider requests. Bash routes through ExecutionProvider. Hosted Sandboxed sessions expose sandbox_exec only; it lazily acquires the injected provider, bounds output, persists oversized output as tenant/session artifacts, and artifact_read can resolve artifact:// IDs. Standard local services install the local-native provider; hosted embedders inject Vercel/E2B/etc providers. Capabilities advertise native vs sandbox execution and lazy acquisition/no native fallback.

Added tenant-owned session control/participants: GET/POST/DELETE /v2/sessions/:id/control and GET participants, short lease, revision CAS, service-account to human takeover on same session, middleware blocks mutations from non-controller during active lease, actor participation recorded. Permission approvals composite tenant/project regression test.

Validation passing: cargo check service-api/server; server --no-run; focused resolver, tenant search/event, takeover, permission collision, and local provider tests. cargo fmt --check currently reports broad pre-existing/unformatted changes across already-dirty files; do not run mutating cargo fmt over whole tree.

Still open: tenant-key workspace/MCP/LSP runtime registries (critical MCP runtime client cache still directory/name keyed); normalize workflows and restore hosted workflow routes; fully migrate background/MCP stdio/LSP/PTY/plugins/ACP/formatters/VCS through ExecutionProvider; implement concrete hosted provider and revisioned workspace materialization/commit; SQL-qualify every artifact/session repository lookup; partition live event bus; expand SDK projects/webhooks/idempotency and end-to-end HTTP tests.
