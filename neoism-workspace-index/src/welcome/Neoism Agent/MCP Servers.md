# MCP Servers

Model Context Protocol servers extend Neoism Agent with tools, resources, and prompts. Neoism supports local subprocess servers and remote HTTP servers, including remote OAuth flows.

## Add a server

The easiest route is **hamburger menu -> Extensions**, find an MCP extension, and choose **Install**. The Extensions page writes the server definition and shows its live status.

To add a custom server manually, put its definition in one of these locations, then start a new agent session:

- Global: `~/.config/neoism/mcp.json` or `~/.config/neoism/config.json` under `agent.mcp`.
- Project: `<project>/neoism.json` under top-level `mcp`.

An `mcp.json` file accepts either `{ "mcp": { "server-name": {...} } }` or the bare `{ "server-name": {...} }` map. Use `/mcp` to confirm whether Neoism connected, disabled, or failed the server.

## Local server

```jsonc
{
  "agent": {
    "mcp": {
      "company-tools": {
        "type": "local",
        "command": ["company-mcp", "serve"],
        "environment": {
          "COMPANY_API_TOKEN": "{env:COMPANY_API_TOKEN}"
        },
        "enabled": true,
        "timeout": 30000
      }
    }
  }
}
```

`command` accepts a string or string array. `args` can be supplied separately. `environment` also accepts the alias `env`. `timeout` is in milliseconds.

## Remote server

```jsonc
{
  "agent": {
    "mcp": {
      "remote-search": {
        "type": "remote",
        "url": "https://mcp.example.com/mcp",
        "headers": {
          "Authorization": "Bearer {env:SEARCH_TOKEN}"
        },
        "oauth": true,
        "enabled": true,
        "timeout": 30000
      }
    }
  }
}
```

`oauth` may be `false`, `true`, or an object containing `client-id`, `client-secret`, `scope`, `redirect-uri`, authorization/token URLs, and a registration URL.

## Lifecycle

Neoism reports each server as:

- `connected`
- `disabled`
- `failed`
- `needs_auth`
- `needs_client_registration`

The MCP UI and server routes can connect, disconnect, start authentication, finish callbacks, and remove stored OAuth state.

## Tools, resources, and prompts

- **Tools** are added to the model's callable inventory with the MCP client identity.
- **Resources** are addressable documents/data exposed by a server.
- **Prompts** are server-provided prompt templates with typed arguments.

Use `/mcp` to inspect configured servers and status.

## Built-in servers

Neoism bundles MCP extensions for:

- **Notes**: indexed Markdown notes, backlinks, tasks, tags, and graph operations.
- **Memory**: project and personal durable memory.
- **Docs**: immutable Neoism product documentation.

These appear as normal tool families but are managed as built-in extensions. They can be disabled/re-enabled through Neoism's extension lifecycle.

## Names and collisions

MCP tool names are qualified by their client integration when necessary. Avoid configuring multiple servers with the same ID. If a tool collides with a native name, Neoism normalizes provider-facing names while preserving the owning client.

## Security

An MCP server can expose side-effecting tools and receive tool inputs. Local commands execute with your user account. Remote headers and OAuth tokens are secrets. Pin trusted executables, use environment substitution, and review permissions before allowing broad MCP access.

See [[Tools and Background Tasks]], [[Permissions]], and [[Troubleshooting]].