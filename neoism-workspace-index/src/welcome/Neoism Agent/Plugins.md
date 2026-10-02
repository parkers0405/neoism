# Plugins

Neoism Agent supports three parallel plugin tiers. Existing plugins do not have to migrate between them.

| Tier | Runtime | Use it for |
|---|---|---|
| Agent Lua | Isolated VM in the supervised `neoism-agent-lua-runner` process | Portable tools and services that fit the sandboxed callback API. |
| `neoism-plugin/2` | Long-lived supervised process using newline-delimited JSON | TypeScript, Python, Go, Rust, or any language that can speak JSON over stdio. |
| Native Rust | Linked into the Agent server | First-party or fully trusted integrations that need direct Rust APIs. |

Editor Lua and editor native/subprocess extensions are separate parallel targets described in [[Plugin Authoring]]. One `neoism-plugin.json` package may declare only `editor`, only `agent`, or both. A dual-target package shares identity and immutable content revision, but its editor and Agent code execute in different runtimes with different grants and no shared globals.

The exhaustive repository references are `docs/plugins.md` for packages, lifecycle and trust; `docs/editor-plugin-api.md` for every editor API; `docs/agent-plugins.md` for Agent Lua, the complete process protocol, TypeScript and native APIs, limits and remote resources; and `docs/lua-api.lua` for generated editor Lua signatures.

## Shared package manifest

User packages live at `~/.config/neoism/plugins/<id>/`. Workspace packages live at `<workspace>/.neoism/plugins/<id>/`. Only immediate child directories containing `neoism-plugin.json` are discovered.

```json
{
  "id": "dev.example.todo",
  "name": "Todo tools",
  "version": "1.0.0",
  "apiVersion": 1,
  "editor": {
    "entrypoint": "editor.lua"
  },
  "agent": {
    "runtime": "lua",
    "entrypoint": "agent.lua",
    "command": [],
    "capabilities": ["workspace-read", "event-publish"],
    "scope": "workspace",
    "eventNamespaces": ["todo"]
  },
  "platforms": ["linux", "macos", "windows"],
  "dependencies": [],
  "triggers": [],
  "capabilities": [],
  "metadata": {}
}
```

Use `editor.entrypoint` for new packages. The legacy top-level `entrypoint` remains supported for editor-only manifests. The different top-level `entrypoints` object belongs to trusted editor extension tiers; it is not the Agent target.

`agent.runtime` is `lua` or `process`. `agent.scope` is `global`, `user`, `workspace`, or `session`. Workspace-located packages may use only workspace or session scope. A process package uses `command` as its executable plus arguments when the array is nonempty; otherwise `entrypoint` names the executable. All entrypoints must remain inside the package.

## Revision, trust, and grants

Discovery is metadata-only. Workspace repository content cannot approve itself and does not execute merely because it contains a package.

Neoism hashes the complete package into an immutable `sha256:<hex>` revision. Hashing rejects symlinks, more than 4,096 files, and more than 64 MiB of content. Changing any package file changes the revision and invalidates exact-revision trust.

Activation requires all applicable layers to agree:

1. The package declares the capability.
2. Installation-owned trust approves the exact package ID, content revision, scope, workspace and optional user/session identity.
3. The host grants the capability in that scope.
4. A concrete broker for the capability exists.

Agent capabilities are `config-read`, `config-write`, `workspace-read`, `workspace-write`, `event-publish`, `network`, `process-spawn`, `task-spawn`, `secret-use`, `secret-read`, `prompt-read`, `message-read`, `response-transform`, `provider-access`, and `policy-invoke`.

Lua needs `luaApproved`; a process needs `executableApproved`; native artifacts need `nativeApproved`. Secret use normally passes an opaque credential alias to a host operation without revealing the credential. Raw secret access requires the separate `secret-read` capability.

Trust and grants are not the same as end-user permission to invoke an Agent tool. Tool permission still runs through the normal permission pipeline after plugin activation.

Lifecycle states include discovered, permission-required, trusted, enabled, loading, active, degraded, failed, disabled, update-available, and restoring. Candidate publication is transactional: a failed update or reload preserves the last-known-good revision and generation.

## Agent Lua

Agent Lua runs outside the Agent server in `neoism-agent-lua-runner`, with one serialized VM per plugin instance. It has no `io`, `os`, `debug`, `dofile`, `loadfile`, unrestricted `load`, native modules, or external module path. `require` resolves only package-contained `lua/?.lua` and `lua/?/init.lua` modules.

The entrypoint returns a table that may declare `name`, `version`, `tools`, `hooks`, `events`, `services`, `routes`, `websocketRoutes`, `messageParts`, and `mcp`. Exact declaration and callback record shapes are listed in `docs/agent-plugins.md`; do not infer them from editor Lua.

`neoism.host` exposes `config_get`, `config_set`, `workspace_read`, `workspace_write`, `workspace_list`, `event_publish`, `network_request`, `process_spawn`, `process_cancel`, `task_spawn`, `task_cancel`, `secret_use`, `secret_read`, `prompt_read`, `message_read`, `response_transform`, `provider_call`, and `policy_call`. Each call is checked against the exact owner generation and effective capabilities.

Runner hard limits include a 16 MiB Lua heap, 1 MiB frames, queue length 64, 250 ms load timeout, 30 second callback timeout, 10 second host-call timeout, 10,000,000 instructions, and 4,096 stream items. Cancellation is cooperative at Lua instruction boundaries.

## Serve plugins: `neoism-plugin/2`

A serve plugin is one long-lived process per package instance and registry generation. It reads and writes one JSON object per line on stdio. Any language may implement the protocol; `@neoism/plugin` is the supported TypeScript author library.

The host first sends `initialize`. The plugin replies with protocol `neoism-plugin/2` and any tools, hooks, event namespaces, agents, commands, skills, configuration, system-context sections, prompts, providers, HTTP routes, WebSocket routes, custom message parts, and MCP metadata/resources/tools it contributes.

Host-to-plugin methods include `initialize`, `tool.invoke`, `hook.invoke`, `agent.list`, `command.list`, `skill.list`, `systemContext.sections`, `prompt.render`, `config.load`, `provider.metadata`, `provider.auth`, `provider.route`, `provider.media`, `provider.stream`, `route.handle`, `route.websocket.open`, `route.websocket.message`, `event`, `$/cancel`, `$/cancelStream`, and `shutdown`.

Plugins can make owner-stamped reverse requests for brokered config, workspace resources, event publication, network, processes, tasks, secrets, prompts, messages, response transforms, providers, and policy. The complete method names, DTOs and capability mapping are in `docs/agent-plugins.md` and `neoism-agent-plugin-api/src/process_v2.rs`.

Provider generation uses bounded open/item/end/error streams. HTTP and WebSocket handlers enter the native route registry. Every request, reverse request and stream is bound to exact plugin ID, process instance, package revision, registry generation, scope and workspace. Late frames from a retired generation are ignored or rejected.

Process limits include an 8 second initialization timeout, 1 MiB frames, 64 KiB log lines, 128 pending calls, 64 queued reverse calls, 32 streams, 64 queued items per stream, 256 KiB stream items, and 16 MiB aggregate stream output. Retirement revokes capabilities, fails streams, sends `shutdown`, waits 1.5 seconds, then kills the child if necessary.

## TypeScript plugin example

```ts
import { definePlugin, runPlugin } from "@neoism/plugin";

await runPlugin(definePlugin({
  tools: [{
    id: "todo_count",
    description: "Count workspace entries",
    parameters: { type: "object", properties: {} },
    async execute(_input, context) {
      const files = await context.host.workspace.list(".");
      return { output: `${files.length} workspace entries` };
    },
  }],
  hooks: {
    "chat.options": (_context, value) => ({ ...value, temperature: 0 }),
  },
  events: {
    namespaces: ["session."],
    handler: (event) => console.error(event.type),
  },
}));
```

Use `@neoism/plugin` for process authoring. Use `@neoism/sdk` for a typed HTTP client of the Agent server. The HTTP SDK is generated from OpenAPI; the process author API is a separate versioned contract with protocol conformance tests.

## Existing configuration-driven plugins

Existing workspace `agent.plugins` entries using `entry`, `npm`, or `serve` remain supported. Existing `neoism-plugin/1` one-request/one-response hook commands also remain supported. Moving either into a shared package is optional and is useful only when the author wants shared editor/Agent identity, immutable package revisions, exact-revision trust, or package lifecycle management.

```jsonc
{
  "agent": {
    "plugins": {
      "dev.example.local": { "options": { "entry": "./plugins/todos" } },
      "dev.example.npm": { "options": { "npm": "@example/neoism-plugin@1.0.0" } },
      "dev.example.custom": { "options": { "serve": ["python3", "plugin.py"] } }
    }
  }
}
```

Options include `config`, `env`, `timeoutMs`, `network`, `sandbox`, and an explicit `capabilities` array. The process receives only the intersection of requested capabilities and host policy.

## Native plugins and remote resources

Native Agent plugins implement `PluginFactory` and `PluginInstance`. They use the same contribution validation and registry publication but run inside the server and therefore are trusted cooperative code, not a sandbox. Native code cannot promise forced thread cancellation or recovery from arbitrary memory corruption. Use the public conformance fixture before shipping one.

Daemon-owned remote files, watches, tasks, tests, PTYs and DAP processes cross the wire only as opaque `pr_<uuid>` capabilities. Never parse, persist, reinterpret as a local path, or transfer a resource ID across owners, sockets, workspaces, revisions or generations. Paths, file descriptors, process IDs, PTY handles and DAP transports do not cross this boundary.

## Inspect and diagnose

`GET /v2/plugins` reports each active plugin's descriptor, contributions, capabilities and health. A plugin failure degrades that plugin instead of taking down the server. Use lifecycle diagnostics to distinguish permission-required, incompatible, failed and retained-last-known-good states.

For exact callback fields, frame envelopes, stream tags, WebSocket records, hard-limit tables, trust records, native lifecycle and source-of-truth links, read `docs/agent-plugins.md`.