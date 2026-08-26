# MCP Servers

Model Context Protocol servers extend Neoism with tools, resources, and prompts. Neoism supports local subprocess servers and remote HTTP servers, including OAuth discovery and dynamic client registration.

## Add a server

The easiest route is **hamburger menu -> Extensions**. Find an MCP extension and choose **Install**. The Extensions page writes its definition and shows live status.

For a custom server, add its definition to one of these locations:

| Scope | File and shape |
|---|---|
| Global application config | `~/.config/neoism/config.json` under `agent.mcp` |
| Global MCP catalog | `~/.config/neoism/mcp.json`, as a bare server map or under `mcp` |
| Project agent config | `<project>/neoism.json` under top-level `mcp` |
| Project MCP catalog | `<project>/.neoism/mcp.json`, as a bare server map or under `mcp` |

Closer project configuration overrides more distant configuration. A dedicated `mcp.json` is merged after ordinary config files in the same configuration directory. `NEOISM_AGENT_CONFIG` and `NEOISM_AGENT_CONFIG_CONTENT` are applied last.

Reopen `/mcp` after editing configuration. Restart Neoism if an existing process still shows an old catalog.

## Local server

This is a bare `mcp.json` example:

```jsonc
{
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
```

`command` accepts a string or string array. `args` can be supplied separately. `environment` also accepts the alias `env`. `timeout` is in milliseconds.

## Remote server with a bearer token

```jsonc
{
  "remote-search": {
    "type": "remote",
    "url": "https://mcp.example.com/mcp",
    "headers": {
      "Authorization": "Bearer {env:SEARCH_TOKEN}"
    },
    "oauth": false,
    "enabled": true
  }
}
```

## Remote server with OAuth

Webflow is a remote OAuth MCP. In `~/.config/neoism/mcp.json`:

```jsonc
{
  "webflow": {
    "type": "remote",
    "url": "https://mcp.webflow.com/mcp",
    "oauth": {},
    "enabled": true
  }
}
```

The equivalent project `neoism.json` wraps the entry under `mcp`:

```jsonc
{
  "mcp": {
    "webflow": {
      "type": "remote",
      "url": "https://mcp.webflow.com/mcp",
      "oauth": true,
      "enabled": true
    }
  }
}
```

OAuth values mean:

- Missing or `false`: OAuth is disabled for this MCP entry.
- `true`: enable OAuth using server metadata discovery and dynamic client registration.
- `{}`: the same default discovery behavior as `true`.
- An object: enable OAuth and optionally provide `clientId`, `clientSecret`, `scope`, `redirectUri`, `authorizationUrl`, `tokenUrl`, or `registrationUrl`.

Use camelCase field names in an OAuth object.

## Manage MCP servers in the agent pane

Run `/mcp` to open the inline MCP picker above the input bar. Each server shows a status dot and label. Select a server with the mouse or press Enter to open its action menu.

Depending on its state, the menu provides:

- **Enable / Disable**: persist the `enabled` value in the configuration file that owns the effective entry. Disable also stops the runtime.
- **Connect / Disconnect**: start or stop the runtime without changing configuration.
- **Connect with OAuth / Reauthenticate**: open the authorization flow for an OAuth MCP.
- **Log out**: remove saved OAuth credentials and disconnect the server.

Press `Esc` in the action menu to return to the MCP list.

## Authenticate from the CLI

The desktop UI and CLI use the same OAuth implementation:

```bash
neoism mcp auth webflow
neoism mcp auth webflow --no-open
neoism mcp auth list
neoism mcp logout webflow
```

`neoism mcp auth <name>` prints the authorization URL, attempts to open it in your browser, and waits for completion. `--no-open` prints the URL without launching a browser. `neoism mcp auth list` lists authentication states. `neoism mcp logout <name>` removes stored credentials.

The default local callback is:

```text
http://127.0.0.1:4096/v2/plugins/dev.neoism.mcp/<name>/auth/callback
```

After authorization, Neoism exchanges the code, stores the credentials, and displays a page that can be closed. A remote daemon or custom deployment may need an explicit externally reachable `redirectUri` in the OAuth object.

## Statuses

- `connected`: the runtime is available.
- `disabled`: configuration has `enabled: false`.
- `needs_auth`: OAuth credentials are missing or expired.
- `needs_client_registration`: the OAuth client must be registered.
- `failed`: startup, transport, configuration, or authentication failed.

## Tools, resources, and prompts

- **Tools** are added to the model's callable inventory with the MCP client identity.
- **Resources** are addressable documents or data exposed by a server.
- **Prompts** are server-provided prompt templates with typed arguments.

## Built-in servers

Neoism bundles MCP extensions for Notes, Memory, and immutable product Docs. They appear as normal tool families and can be disabled or re-enabled through the MCP or extension lifecycle.

## Security

An MCP server can expose side-effecting tools and receive tool inputs. Local commands execute with your user account. Remote headers and OAuth tokens are secrets. Pin trusted executables, use environment substitution, and review permissions before allowing broad MCP access.

See [[Tools and Background Tasks]], [[Permissions]], and [[Troubleshooting]].