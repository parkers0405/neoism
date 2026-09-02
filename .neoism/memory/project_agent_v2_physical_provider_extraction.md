---
name: "Agent V2 physical provider extraction"
description: "Physical provider implementation extraction into server-independent builtins via ProviderService"
type: "project"
scope: "project"
origin: "neoism_agent_v2 implementation session"
created: "2026-08-25"
updated: "2026-08-25"
---

On branch neoism_agent_v2, concrete model provider runtime implementations, model catalog, provider auth/OAuth, response parsing, request transforms, and provider auth store were physically moved out of neoism-agent-server into neoism-agent-builtins. ProviderPlatform implements plugin-api ProviderService, including typed model metadata/auth/provider route contracts. ProvidersPlugin now routes directly through ProviderService; the server Provider/ProviderAdmin adapters and provider_routes module were removed. AppState owns only Arc<dyn ProviderService>, not concrete catalog/auth/OAuth/registry fields. Server provider source now retains only kernel stream message/processor orchestration. Windows secret ACL helper moved to builtins and remains shared by MCP auth. No cargo/rustc/tests/rust-analyzer/format/git commands were run per user constraint.
