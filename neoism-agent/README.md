# Neoism agent framework

Build with the same agent framework that powers Neoism's local-first IDE. Neoism includes its own embeddable Rust runtime, TypeScript SDK, and plugin framework for agent-powered applications and workflows.

The runtime brings together sessions, streaming events, provider integrations, tool execution, permissions, persistence, and MCP support. The desktop application and workspace daemon host it directly. The `neoism-agent serve` executable hosts the same runtime for Docker, remote hosts, and other headless deployments.

Local hosting keeps the runtime and workspace execution on your machine; inference uses your configured model provider. For the product overview and local-first boundaries, see the [workspace guide](../docs/README.md).

## Build on the platform

- Use the [TypeScript SDK](sdk/typescript/README.md) to connect your application to the Agent API, manage sessions, and consume streaming events.
- Write [native Rust or supervised process plugins](../docs/plugins.md) to add tools, hooks, providers, and integrations. `@neoism/plugin` supplies the TypeScript process-plugin authoring API.
- Embed `neoism-agent-server` in a Rust host, or deploy it with `neoism-agent serve`.
- Consult the [Agent reference](../docs/agent-plugins.md) for extension contracts, capability discovery, permissions, and lifecycle details.

## Crates

- `neoism-agent-core`: protocol models, IDs, events, sessions, messages, parts,
  tools, permissions, provider metadata, and plugin contracts.
- `neoism-agent-server`: HTTP/SSE runtime, provider integrations, MCP support,
  persistence, tools, language services, and embedded server entrypoints.
- `neoism-agent`: minimal `serve`-only launcher for the same server runtime.

User-facing provider and MCP setup commands live on the GUI executable:

```bash
neoism auth login codex
neoism auth list
neoism auth logout openai
neoism mcp list
neoism mcp auth supabase
neoism mcp logout supabase
```

The runtime remains embeddable through `neoism-agent-server`. Use `neoism auth` and `neoism mcp` for provider and MCP setup; `neoism-agent` is the server-hosting executable.

For shared multi-tenant deployments and standalone cloud workers, start with [Hosted control plane and isolated workspace workers](docs/hosted-control-plane.md). The shared control plane is execution-disabled and embeds the server with a tenant resolver, shared artifact store, and scoped credential stores. Execution uses the same stock Agent engine inside one isolated VM per logical workspace, managed outside tools by a whole-workspace infrastructure provider. The [workspace host](../neoism-cloud-host/README.md) supplies launch preparation, controller signing, lifecycle orchestration and authenticated connection brokerage; the [cloud SDK guide](sdk/typescript/CLOUD.md) separates application clients from infrastructure clients. The Docker development provider is implemented and tested, but is container isolation, not a production VM guarantee.