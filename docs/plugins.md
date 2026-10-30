# Neoism plugin authoring

On the GPUIX branch, Agent extensions use native Rust plugins or supervised process plugins. MCP servers and managed LSP/Tree-sitter/kernel packages remain separate daemon features. The editor Lua host, Agent Lua runner, `init.lua`, and Lua plugin installation/approval UI are removed.

## Boundaries

The daemon owns workspace services and Agent execution. GPUIX owns the window and paints client UI. `neoism-customization` contains retained style/contribution types, not an executable editor plugin API. A package cannot install code into the frontend render or input path.

Native Agent plugins implement the `neoism-agent-plugin-api` contracts and are composed into the server. Process plugins speak `neoism-plugin/2` over newline-delimited JSON on standard input/output. The TypeScript `@neoism/plugin` package implements that protocol. Process failures are isolated from the workspace generation.

## Process packages

Packages live in the user's `neoism/plugins` configuration directory or `.neoism/plugins` in a workspace. Each package has a `neoism-plugin.json` manifest:

```json
{
  "id": "dev.example.todos",
  "name": "Todos",
  "version": "1.0.0",
  "agent": {
    "runtime": "process",
    "entrypoint": "plugin.mjs",
    "command": ["node", "plugin.mjs"],
    "capabilities": ["workspace-read"],
    "scope": "workspace"
  }
}
```

A nonempty `command` supplies the executable and arguments. Otherwise `entrypoint` names a contained executable relative to the package. The package revision is content-derived. Runtime values of `lua` are rejected; an omitted runtime defaults to `process`.

Agent scopes are `global`, `user`, `workspace`, and `session`. Workspace-located packages may declare only workspace/session scope. Explicit approval grants capabilities to an exact package revision and owner scope; process execution requires `executableApproved`. Native artifacts require their native approval boundary. Removing Lua does not bypass process sandboxing, credentials, or capability grants.

Native and process plugins keep the existing lifecycle, service registry, tools, hooks, providers, routes, prompts, skills, scoped configuration, and opaque remote resources. See [the exhaustive Agent reference](agent-plugins.md) for protocol frames, SDK examples, limits, lifecycle behavior, and resource operations.

## Source contracts

- `neoism-agent/crates/neoism-agent-plugin-api`: package, lifecycle, broker, and process DTOs.
- `neoism-agent/crates/neoism-agent-server/src/plugin_host_process.rs`: supervised process host.
- `neoism-agent/crates/neoism-agent-server/src/plugin_package.rs`: discovery, revision, compatibility, scope, and approval checks.
- `neoism-agent/sdk/typescript/packages/plugin`: TypeScript process host SDK.

Use `bash scripts/blaze.sh test -p neoism-agent-plugin-api` and the relevant server/SDK tests when changing these contracts. `bash scripts/check-frontend-deps.sh` prevents reintroducing retired Lua or renderer dependencies into the frontend or daemon.
