# Neoism agent runtime

The Neoism agent is an embeddable, headless Rust runtime. The desktop application
and workspace daemon host it directly. A minimal `neoism-agent serve` executable
is also shipped for Docker, remote hosts, and other headless deployments; it does
not include the former chat/TUI/session/tool CLI.

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

The runtime remains embeddable through `neoism-agent-server`; provider and MCP
setup stay on the small `neoism auth` command surface, while `neoism-agent` stays
limited to hosting the server.

For shared multi-tenant deployments and standalone cloud workers, start with [Hosted control plane and isolated workspace workers](docs/hosted-control-plane.md). The shared control plane is execution-disabled and embeds the server with a tenant resolver, shared artifact store, and scoped credential stores. Execution uses the same stock Agent engine inside one isolated VM per logical workspace, managed outside tools by a whole-workspace infrastructure provider. The [workspace host](../neoism-cloud-host/README.md) supplies launch preparation, controller signing, lifecycle orchestration and authenticated connection brokerage; the [cloud SDK guide](sdk/typescript/CLOUD.md) separates application clients from infrastructure clients. The Docker development provider is implemented and tested, but is container isolation, not a production VM guarantee.