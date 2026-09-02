---
name: "Agent V2 physical agent catalog extraction"
description: "Physical agent catalog/native prompt extraction into builtins-owned AgentsPlugin contracts"
type: "project"
scope: "project"
origin: "neoism_agent_v2 implementation session"
created: "2026-08-25"
updated: "2026-08-25"
---

On branch neoism_agent_v2, physical agent catalog ownership was moved from neoism-agent-server into neoism-agent-builtins/plugin/agents. Builtins now owns AgentCatalog, native agent definitions/prompts, config merge/default interpretation, and catalog tests. AgentsPlugin constructs a built-in source from AgentServices and registers both public AgentSource and AgentService contracts; its list/get routes use the service. RegistrySnapshot/PluginRegistrar now retain agent_services. Server transitional Agents adapter and server agent.rs/agent_native.rs were removed. Server consumers resolve only through a pinned RegistrySnapshot AgentSourceSnapshot, whose public list/get/default_agent methods preserve the former runtime view; no direct config fallback was added. No cargo/rustc/tests/rust-analyzer/format/git commands were run per user constraint.
