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