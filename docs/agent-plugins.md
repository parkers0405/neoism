# Agent plugin authoring

This document describes the Agent extension contracts implemented in this repository: shared `neoism-plugin.json` packages, sandboxed Agent Lua, the language-neutral `neoism-plugin/2` process protocol, `@neoism/plugin`, the trusted native Rust tier, and the separate daemon-owned remote-resource protocol.

Read [`plugins.md`](plugins.md) first for the shared tier and package model. Editor Lua and editor extension APIs are documented separately in [`editor-plugin-api.md`](editor-plugin-api.md) and generated [`lua-api.lua`](lua-api.lua); Agent Lua is not the editor Lua runtime and does not expose those namespaces.

The wire names below are exact. Manifest and protocol DTOs use `camelCase` unless a table explicitly says otherwise; capability and scope enum values use `kebab-case`; HTTP methods use uppercase strings; provider stream event tags use `kebab-case`; and remote-resource request tags use `snake_case`.

## Choosing a tier

| Tier | Declaration | Execution boundary | Intended use |
| --- | --- | --- | --- |
| Agent Lua | `agent.runtime: "lua"` | Dedicated `neoism-agent-lua-runner` subprocess with an isolated Lua VM | Portable third-party plugins that fit the declared callback model |
| Process | `agent.runtime: "process"` | Long-lived supervised subprocess speaking NDJSON `neoism-plugin/2` | TypeScript, Python, Go, Rust, or any other language with stdio and JSON |
| Native Rust | Host-linked `PluginFactory`/`PluginInstance` | In the Agent server process | First-party or otherwise fully trusted cooperative code only |

Lua and process packages go through the same process adapter and registry lifecycle after initialization; native plugins use the same contribution registry but do not have a security boundary from the host.

## Shared package manifest

Every discovered package is a directory containing a file named exactly `neoism-plugin.json`. Parsing is metadata-only: discovery does not evaluate Lua, start a process, or resolve an editor entrypoint.

### Complete manifest shape

```json
{
  "id": "dev.example.todo",
  "name": "Todo tools",
  "version": "1.0.0",
  "apiVersion": 1,
  "editor": null,
  "agent": {
    "runtime": "lua",
    "entrypoint": "agent.lua",
    "command": [],
    "capabilities": ["config-read", "workspace-read", "event-publish"],
    "scope": "workspace",
    "eventNamespaces": ["todo"]
  },
  "platforms": [],
  "dependencies": [],
  "triggers": [],
  "capabilities": [],
  "metadata": {}
}
```

| Field | Exact type and default | Agent meaning |
| --- | --- | --- |
| `id` | string, default `""` | Required in practice; Agent package discovery requires a lowercase reverse-DNS identifier as described below. |
| `name` | string, default `""` | Display name. |
| `version` | string, default `""` | Package-authored version label; revision trust is based on content, not this value. |
| `apiVersion` | unsigned integer, default `1`; `api_version` is accepted as an input alias | Shared package API marker. The current Agent package loader does not use this field for process protocol negotiation. |
| `entrypoint` | string or `null`, default `null` | Backward-compatible editor-only entrypoint; it is not an Agent fallback. New packages should use `editor.entrypoint`. |
| `editor` | `{ "entrypoint": string }` or `null`, default `null` | Canonical editor target. The editor executes this path when present. |
| `agent` | Agent object or `null`, default `null` | Presence makes the package an Agent package candidate. |
| `platforms` | string array, default `[]` | Shared portable metadata; the current Agent package loader does not filter on it. |
| `dependencies` | string array, default `[]` | Shared portable metadata; the current Agent package loader does not turn it into native registry dependencies. |
| `triggers` | JSON array, default `[]` | Editor-portable opaque metadata. |
| `capabilities` | JSON array, default `[]` | Editor-portable opaque metadata; Agent authority comes from `agent.capabilities`. |
| `metadata` | object of JSON values, default `{}` | Package-defined metadata. |

The `agent` object has these exact fields and defaults.

| Field | Exact values and default | Behavior |
| --- | --- | --- |
| `runtime` | `"lua"` or `"process"`; default `"lua"` | Chooses the Lua runner or an arbitrary process. |
| `entrypoint` | string; default `""` | For Lua it must name the contained Lua file. For process runtime it is used as the executable when `command` is empty. |
| `command` | string array; default `[]` | For process runtime, a nonempty array is the executable followed by its arguments and takes precedence over `entrypoint`. |
| `capabilities` | array of the exact capability strings below; default `[]` | Requested host authority. Every requested capability must be present in the exact trust record and in the host's scope grants. |
| `scope` | `"global"`, `"user"`, `"workspace"`, or `"session"`; default `"workspace"` | Exact runtime instance scope. |
| `eventNamespaces` | string array; default `[]` | Manifest metadata currently carried into the package DTO; runtime event subscriptions are declared by the Lua/process initialize response. |

The complete Agent capability vocabulary is `config-read`, `config-write`, `workspace-read`, `workspace-write`, `event-publish`, `network`, `process-spawn`, `task-spawn`, `secret-use`, `secret-read`, `prompt-read`, `message-read`, `response-transform`, `provider-access`, and `policy-invoke`.

Package IDs are more restrictive than native registry IDs: they must contain at least two dot-separated parts; each part must begin with a lowercase ASCII letter; remaining bytes may be lowercase ASCII letters, digits, or `-`; and a part may not end in `-`. For example, `dev.example.todo` is valid and `Example.todo`, `todo`, and `dev.example.todo_1` are not.

For Lua, and for a process package with an empty `command`, `entrypoint` must be nonempty, relative, contain only normal path components, resolve to a regular file, and remain under the canonical package root. A process package with a nonempty `command` is not subjected to entrypoint validation.

### Discovery roots and precedence

Agent discovery scans exactly these roots:

1. `<platform config directory>/neoism/plugins` with location `"user"`.
2. `<workspace>/.neoism/plugins` with location `"workspace"`.

Only immediate child directories containing `neoism-plugin.json` are candidates; discovery does not recursively search arbitrary descendants. Entries are sorted by directory name within each root, and the user root is visited before the workspace root.

Workspace packages may declare only `"workspace"` or `"session"` Agent scope. `"global"` and `"user"` activation is restricted to packages from the user/installation root. A session package still binds to an exact workspace and session identity.

Plugin configuration and trust must come from an installation-scoped configuration layer; a repository-owned workspace configuration cannot make its own code trusted. The effective configuration entry is keyed by package ID in the installation `plugins` map.

### Content revision

The package revision is `sha256:<lowercase hex>`. The hasher recursively walks the entire package root, rejects every symlink, rejects more than 4,096 regular files, sorts file paths, and rejects more than 64 MiB of total file content.

For each sorted file the digest receives the root-relative path bytes as represented by `to_string_lossy()`, one `0x00` byte, the full file bytes, and one `0xff` byte. Any file change, path change, addition, or deletion therefore changes the revision; the authored `version` does not preserve approval.

### Exact trust record

The installation plugin entry must be enabled and its `options.trust` value must deserialize to this shape.

```json
{
  "plugins": {
    "dev.example.todo": {
      "enabled": true,
      "options": {
        "config": { "label": "TODO" },
        "timeoutMs": 60000,
        "sandbox": true,
        "trust": {
          "workspaceId": "/absolute/workspace/identity",
          "packageId": "dev.example.todo",
          "revision": "sha256:REPLACE_WITH_DISCOVERED_REVISION",
          "scope": "workspace",
          "scopeId": null,
          "capabilities": ["config-read", "workspace-read", "event-publish"],
          "executableApproved": false,
          "nativeApproved": false,
          "luaApproved": true
        }
      }
    }
  }
}
```

`workspaceId`, `packageId`, and `revision` are required strings. `scope` is optional but, when present, must equal the activated scope. `scopeId` is the exact opaque user or session identity and is required to match for `"user"` and `"session"` activation. `capabilities` defaults to `[]`; `executableApproved`, `nativeApproved`, and `luaApproved` default to `false`.

Lua requires `luaApproved: true`; process runtime requires `executableApproved: true`. `nativeApproved` is represented in the shared trust DTO but does not turn a package into an in-process Agent native plugin.

For workspace and session scopes the trust record's `workspaceId` must exactly match the runtime workspace identity. Global and user packages must come from the user/installation package root. User and session trust is additionally pinned to `scopeId`.

### Capability intersection

Authority is the intersection of four conditions: the package requests a capability in `agent.capabilities`; the exact revision trust record approves it; the host grants it for that runtime scope; and the host has installed the corresponding concrete service or broker. Naming a capability never manufactures a service.

Activation rejects the candidate before lifecycle side effects if any requested capability is missing from the host context. After preflight, the context passed to the plugin is attenuated to exactly its requested capabilities and gets a generation-local revocation lease.

The current production workspace grant set names `config-read`, `config-write`, `workspace-read`, `workspace-write`, `event-publish`, `network`, `process-spawn`, and `secret-read`. The latter capabilities also admit trusted built-in descriptors during registry construction; an external plugin call still requires the corresponding concrete scoped broker and fails when none is installed. The configuration snapshot is read-only, so `host.config.set` returns an error. Current global, user, and session package grants contain only `config-read` and `event-publish`. Other broker capabilities require a host integration that explicitly installs that broker.

Workspace operations accept only contained relative paths. Reads and lists canonicalize the target; writes require a canonical contained parent and re-check existing targets, preventing absolute paths, parent traversal, and symlink escape.

### Lifecycle and generations

Discovery begins in `"discovered"`, `"incompatible"`, `"permission-required"`, `"trusted"`, or `"disabled"` depending on metadata, compatibility, configuration, and exact trust. Runtime reporting can additionally expose `"enabled"`, `"loading"`, `"active"`, `"degraded"`, `"failed"`, `"update-available"`, and `"restoring"`.

Each exact global/user/workspace/session key owns an independently revocable generation. A candidate is fully created, started, checked for `"ready"` or `"degraded"` readiness, validated, and converted to an immutable registry snapshot before publication. A failed candidate does not displace the last-known-good generation.

Generation numbers are monotonic attempt identities, not counts of successful publications. Owner metadata is stamped by the host, contribution conflicts fail installation, higher service priority is considered first, and equal priorities are ordered by stable ID rather than registration order.

When a new generation publishes, the prior generation is shut down. Shutdown first marks the snapshot inactive and revokes every capability lease, then calls plugin shutdown in reverse dependency order. Scoped deactivation removes only the exact scope key; session teardown therefore does not remove another session's generation.

If newly discovered content has a different revision while the old active revision remains installed, lifecycle reports `"update-available"`, `retainedRevision`, and an active lease. The changed content needs a new exact approval before it can replace the retained generation.

`GET /v2/plugins/lifecycle?directory=...` exposes `PackageLifecycleInfo`: `packageId`, `state`, `source` (`"user"` or `"workspace"`), `revision`, `scope`, optional `scopeId`, `requestedCapabilities`, `grantedCapabilities`, optional `retainedRevision`, `leaseActive`, and diagnostics containing `code`, `message`, and optional `action`.

## Agent Lua authoring

An Agent Lua entrypoint is a Lua chunk that must return one declaration table. Lua executes only in `neoism-agent-lua-runner`; it is not linked into the Agent server, renderer, daemon, worker, or web runtime.

### Copy-ready Lua package

`neoism-plugin.json`:

```json
{
  "id": "dev.example.todo",
  "name": "Todo tools",
  "version": "1.0.0",
  "agent": {
    "runtime": "lua",
    "entrypoint": "agent.lua",
    "capabilities": ["config-read", "workspace-read", "event-publish"],
    "scope": "workspace",
    "eventNamespaces": []
  }
}
```

`agent.lua`:

```lua
local neoism = require("neoism")

return {
  name = "Todo tools",
  version = "1.0.0",

  tools = {
    {
      id = "todo_count",
      description = "Count TODO markers in a UTF-8 workspace file",
      parameters = {
        type = "object",
        properties = { path = { type = "string" } },
        required = { "path" },
        additionalProperties = false,
      },
      outputSchema = { type = "object" },
      execute = function(call)
        local bytes = neoism.host.workspace_read({ path = call.input.path })
        local chars = {}
        for i, byte in ipairs(bytes) do chars[i] = string.char(byte) end
        local text = table.concat(chars)
        local count = 0
        for _ in string.gmatch(text, "TODO") do count = count + 1 end
        return { title = "TODO count", output = tostring(count), metadata = { count = count } }
      end,
    },
  },

  hooks = {
    ["chat.options"] = function(call)
      local value = call.value
      value.temperature = 0
      return value
    end,
  },

  events = {
    namespaces = { "session." },
    handler = function(event)
      neoism.host.event_publish({
        namespace = "todo",
        name = "observed",
        payload = { sourceType = event.type },
      })
    end,
  },

  services = {
    systemContext = {
      {
        id = "todo.context",
        priority = 10,
        sections = function(request)
          return { { id = "todo", title = "Todo policy", content = "Resolve TODOs explicitly." } }
        end,
      },
    },
  },
}
```

The `neoism` global and `require("neoism")` return the same table. The entrypoint may use package-local modules under `lua/<module>.lua` or `lua/<module>/init.lua` with `require("module")`.

### Returned table declarations and callback shapes

| Returned key | Declaration and callback |
| --- | --- |
| `name`, `version` | Optional strings copied into the initialize response. |
| `tools` | Sequence of `{ id, description?, parameters?, outputSchema?, execute }`. `execute(call)` receives `{ tool, directory, sessionId?, input }` and returns `{ title, output, metadata? }`; `output` must be JSON-serializable. Missing description becomes `""`, invalid/missing parameters become `{ "type": "object" }`, and invalid/missing `outputSchema` becomes JSON `null`. |
| `hooks` | Map from hook name to function. `handler(call)` receives `{ hook, context, value }` and returns the replacement JSON value. |
| `events` | `{ namespaces?, handler? }`. `handler(event)` receives the serialized Agent event notification; its return value is ignored. |
| `services.agents` | Sequence of `{ id, priority?, list }`. `list(ServiceRequest)` returns `AgentCatalog`. |
| `services.commands` | Sequence of `{ id, priority?, list }`. `list(ServiceRequest)` returns a `CommandInfo[]`. |
| `services.skills` | Sequence of `{ id, priority?, list }`. `list(ServiceRequest)` returns a `SkillInfo[]`. |
| `services.systemContext` | Sequence of `{ id, priority?, sections }`. `sections(ServiceRequest)` returns `SystemContextSection[]`. |
| `services.prompts` | Sequence of `{ id, priority?, render }`. `render(PromptRequest)` returns `RenderedPrompt`. |
| `services.config` | Sequence of `{ id, priority?, load }`. `load(ServiceRequest)` returns `ConfigDocument`. |
| `services.providers` | Sequence of `{ descriptor, priority?, stream, metadata?, auth?, route?, media? }`. Provider callbacks receive the complete method params described in the process protocol. `stream` returns an array of provider stream events. `media` presence declares media support; `route` presence declares administration support. |
| `routes` | Sequence of `{ descriptor, priority?, handle }`. `handle(RouteRequest)` returns `RouteResponse`. |
| `websocketRoutes` | Sequence of `{ descriptor, priority?, open, message? }`. `open(RouteRequest)` returns `nil` or an array of outbound WebSocket messages; `message(ProcessWebSocketMessage)` returns `nil` or another outbound array. |
| `messageParts` | Array copied directly to the initialize response; each item must match `{ id, version, schema, fallbackTextField? }`. There is no message-part callback. |
| `mcp` | Array copied directly to the initialize response; each item must match `{ id, transport?, configSchema?, authentication?, prompts?, resources?, tools? }`. This is metadata only. |

`priority` defaults to `0`. Provider IDs come from `descriptor.id`; route and WebSocket IDs come from `descriptor.id`. Missing required IDs or callbacks fail Lua plugin loading.

### Every `neoism.host` function

Lua host functions each take exactly one JSON-serializable Lua value, block for a reply, and either return the decoded result or raise a Lua runtime error. Broker functions return the host's `{ output = ... }` envelope; unlike the TypeScript convenience API, Lua does not unwrap `output`.

| Lua function | Reverse method | Required capability | Input and result |
| --- | --- | --- | --- |
| `config_get({ key = string })` | `host.config.get` | `config-read` | Returns the JSON value or `nil`. An empty key asks the current snapshot implementation for the whole document. |
| `config_set({ key = string, value = any })` | `host.config.set` | `config-write` | Returns `{}` on success; the production snapshot grant is read-only and errors. |
| `workspace_read({ path = string })` | `host.workspace.read` | `workspace-read` | Returns a Lua sequence of byte integers. |
| `workspace_write({ path = string, contents = { byte, ... } })` | `host.workspace.write` | `workspace-write` | Returns `{}`. |
| `workspace_list({ path = string })` | `host.workspace.list` | `workspace-read` | Returns a sorted string sequence. |
| `event_publish({ namespace = string, name = string, payload = any })` | `host.event.publish` | `event-publish` | Returns `{}`; namespace and name must remain nonempty after trimming `.`. The emitted Agent event type is `plugin.<namespace>.<name>`. |
| `network_request({ operation = string, input = any? })` | `host.network.request` | `network` | Returns `{ output = any }`. |
| `process_spawn({ operation = string, input = any? })` | `host.process.spawn` | `process-spawn` | Returns `{ output = any }`, normally containing only host-defined opaque data. |
| `process_cancel({ opaqueId = string })` | `host.process.cancel` | `process-spawn` | Returns `{}`. |
| `task_spawn({ operation = string, input = any? })` | `host.task.spawn` | `task-spawn` | Returns `{ output = any }`. |
| `task_cancel({ opaqueId = string })` | `host.task.cancel` | `task-spawn` | Returns `{}`. |
| `secret_use({ operation = string, input = any? })` | `host.secret.use` | `secret-use` | Returns `{ output = any }`; the contract is for using an opaque credential, not receiving a bearer secret. |
| `secret_read({ operation = string, input = any? })` | `host.secret.read` | `secret-read` | Returns `{ output = any }` when an explicitly installed broker permits it. |
| `prompt_read({ operation = string, input = any? })` | `host.prompt.read` | `prompt-read` | Returns `{ output = any }`. |
| `message_read({ operation = string, input = any? })` | `host.message.read` | `message-read` | Returns `{ output = any }`. |
| `response_transform({ operation = string, input = any? })` | `host.response.transform` | `response-transform` | Returns `{ output = any }`. |
| `provider_call({ operation = string, input = any? })` | `host.provider.call` | `provider-access` | Returns `{ output = any }`. |
| `policy_call({ operation = string, input = any? })` | `host.policy.call` | `policy-invoke` | Returns `{ output = any }`. |

Broker `operation` must be 1–256 bytes as measured by Rust string length. Cancellation `opaqueId` must also be 1–256 bytes. Operations and inputs are host-defined; there is deliberately no generic path, token, provider credential, PID, file descriptor, or socket contract.

### Lua containment

The runner creates a fresh Lua VM, installs the host bridge, then evaluates the entrypoint. `io`, `os`, `debug`, `dofile`, `loadfile`, and `load` are removed. `package.path` and `package.cpath` are empty, `package.loadlib` is removed, and searchers after the contained Lua searcher are removed, so native module loading and ambient system modules are unavailable.

Module names must be nonempty dot-separated ASCII alphanumeric/underscore components. Modules resolve only beneath the package's canonical `lua` directory, must be regular files, and may not themselves be symlinks. The Agent entrypoint is likewise canonicalized and must remain beneath the package root.

Host calls carry the exact owner received during initialization and time out after 10 seconds. The server never gives the Lua process its Agent bearer token.

### Lua hard limits

| Limit | Exact value |
| --- | --- |
| Serialized stdin or stdout frame | 1,048,576 bytes |
| Input queue | 64 frames |
| Lua memory limit | 16 MiB |
| Entrypoint load time | 250 ms |
| One callback time | 30 s |
| One host reverse call | 10 s |
| Instructions per load/callback budget | 10,000,000, checked every 1,000 instructions |
| Provider stream result array | 4,096 items |
| WebSocket open/message result array | 4,096 messages |

`$/cancel`, `$/cancelStream`, and `shutdown` set the runner's cancellation flag. The instruction hook turns that flag into `callback cancelled`; it cannot interrupt time spent blocked outside Lua instruction execution. Oversized or malformed inbound lines are skipped by the reader, and a full input queue terminates the reader loop.

## The language-neutral `neoism-plugin/2` protocol

### Transport and frame grammar

The host starts one long-lived process per plugin generation with piped stdin, stdout, and stderr. Stdin and stdout carry UTF-8 NDJSON: one JSON object per line, with `\n` framing and optional preceding `\r`. Do not write logs to stdout; use stderr.

The host sets `NEOISM_PLUGIN_ID`, `NEOISM_PLUGIN_PROTOCOL=neoism-plugin/2`, `NEOISM_PLUGIN_INSTANCE_ID`, and `NEOISM_WORKSPACE_DIR`. It removes `NEOISM_AGENT_TOKEN`. If the parent has `NEOISM_SERVER`, the child receives that value as `NEOISM_AGENT_SERVER_URL`, but no bearer credential is inherited.

Host-to-plugin request or notification:

```text
Request      = { "id"?: uint64|null, "method": string, "params": JSON }
Request      = Request "\n"
```

An `id` denotes a request and requires one terminal reply. Missing or `null` `id` denotes a notification and must not be replied to.

Plugin-to-host terminal reply:

```text
Reply        = { "id": uint64, "result": JSON }
             | { "id": uint64, "error": string }
Reply        = Reply "\n"
```

Plugin-to-host reverse request:

```text
Reverse      = { "id": uint64, "method": string, "params": JSON, "owner": Owner }
Owner        = {
                 "pluginId": string,
                 "instanceId": string,
                 "packageRevision"?: string,
                 "registryGeneration"?: uint64,
                 "scope"?: "global"|"user"|"workspace"|"session",
                 "workspaceId"?: string,
                 "scopeId"?: string
               }
```

The host replies to a reverse request with the same terminal reply grammar. Host and plugin request IDs occupy independent sequences; direction is determined by the presence of `method` on plugin output.

Plugin stream frame:

```text
StreamFrame  = { "owner": Owner, "requestId": uint64, "stream": Stream }
Stream       = { "kind": "open",  "streamId": string }
             | { "kind": "item",  "streamId": string, "value": JSON }
             | { "kind": "end",   "streamId": string }
             | { "kind": "error", "streamId": string, "error": string }
```

Every reverse request and stream frame must echo the complete `owner` object from `initialize.params.owner` exactly, including absent versus present optional fields. The host rejects requests from the wrong owner or a retired instance and silently ignores stream frames that fail owner validation.

### Initialize

The first request is `initialize`, with an 8-second host timeout.

```json
{
  "id": 1,
  "method": "initialize",
  "params": {
    "protocol": "neoism-plugin/2",
    "pluginId": "dev.example.todo",
    "instanceId": "plugin-instance-7",
    "directory": "/workspace/package-root",
    "config": { "label": "TODO" },
    "owner": {
      "pluginId": "dev.example.todo",
      "instanceId": "plugin-instance-7",
      "packageRevision": "sha256:...",
      "registryGeneration": 12,
      "scope": "workspace",
      "workspaceId": "/workspace"
    }
  }
}
```

`owner` is optional in the DTO for additive compatibility but current package-hosted processes receive it. `directory` is the process package working directory, not an authority token. `config` is `options.config` or `{}`.

The reply result has this complete declaration shape; fields shown with `?` may be omitted and all collection fields default to empty.

```json
{
  "protocol": "neoism-plugin/2",
  "name": "Example",
  "version": "1.0.0",
  "tools": [
    {
      "id": "todo_count",
      "description": "Count TODO markers",
      "parameters": { "type": "object" },
      "outputSchema": null,
      "permission": null
    }
  ],
  "hooks": ["chat.options"],
  "eventNamespaces": ["session."],
  "services": {
    "agents": [{ "id": "example.agents", "priority": 0 }],
    "commands": [],
    "skills": [],
    "systemContext": [],
    "prompts": [],
    "config": [],
    "providers": [
      {
        "descriptor": { "id": "example.provider", "name": "Example", "models": [], "configSchema": null },
        "priority": 0,
        "media": false,
        "administration": false
      }
    ]
  },
  "routes": [],
  "websocketRoutes": [],
  "messageParts": [],
  "mcp": []
}
```

A tool definition is `{ id, description, parameters, outputSchema?, permission? }`; `permission`, when present, is `{ permission: string, argument: string }`. A unary service declaration is `{ id: string, priority: int32 }`. A provider declaration is `{ descriptor, priority, media, administration }`, where the descriptor is `{ id, name, models, configSchema? }`.

The initialize `protocol` reply must be exactly `"neoism-plugin/2"`. Unknown additive response fields are ignored by Serde, but authors should not rely on unrecognized fields having an effect.

### Every host-to-plugin method

| Method | `params` | Required reply/result |
| --- | --- | --- |
| `initialize` | `ProcessInitializeRequest` above | `ProcessInitializeResponse` above. |
| `shutdown` | `{}` | Reply `{}` when `id` is present, stop accepting work, and exit. The host also accepts process exit. |
| `event` | Serialized `EventPayload`: `{ id, type, sequence?, properties... }` according to the core event serializer | Notification; no reply. Sent only through the registered event bridge. |
| `tool.invoke` | `{ tool, directory, sessionId?, input }` | `{ title: string, output: string-or-JSON, metadata?: JSON }`. Host defaults missing title to the tool ID, stringifies non-string output, and treats missing output as `""`. |
| `hook.invoke` | `{ hook, context, value }` | The replacement JSON value. |
| `agent.list` | `{ serviceId, request: ServiceRequest }` | `AgentCatalog`. |
| `command.list` | `{ serviceId, request: ServiceRequest }` | `CommandInfo[]`. |
| `skill.list` | `{ serviceId, request: ServiceRequest }` | `SkillInfo[]`. |
| `systemContext.sections` | `{ serviceId, request: ServiceRequest }` | `SystemContextSection[]`. |
| `prompt.render` | `{ serviceId, request: PromptRequest }` | `RenderedPrompt`. |
| `config.load` | `{ serviceId, request: ServiceRequest }` | `ConfigDocument`. |
| `provider.metadata` | `{ serviceId, model: UserModel }` | `ProviderModelMetadata`. |
| `provider.auth` | `{ serviceId, providerId }` | `AuthInfo` or `null`. |
| `provider.route` | `{ serviceId, request: ProviderRouteRequest }` | `RouteResponse`. |
| `provider.media` | `{ serviceId, kind, providerId, modelId, connectionId, tenantId, workspaceId, prompt, options }` | `{ mime, filename, dataBase64, revisedPrompt }`; nullable Rust fields are still serialized as JSON `null` because they are not skipped. |
| `provider.stream` | `{ serviceId, streamId, requestId, request: ProviderGenerationRequest }` | First reply `{}` to accept, then stream frames tied to `streamId` and `requestId`. |
| `route.handle` | `{ routeId, request: RouteRequest }` | `RouteResponse`. |
| `route.websocket.open` | `{ routeId, streamId, requestId, request: RouteRequest }` | First reply `{}` to accept, then outbound WebSocket messages in stream `item` values. |
| `route.websocket.message` | `{ streamId, message: ProcessWebSocketMessage }` | May be a notification; if the host sends an `id`, reply `{}`. |
| `$/cancel` | `{ id: uint64 }` | Notification; cancel the matching in-flight unary call. |
| `$/cancelStream` | `{ streamId, requestId }` | Notification; cancel and close the matching provider/WebSocket stream. |

### Unary service DTOs

`ServiceRequest` is `{ workspaceId?: string|null, directory?: string|null, options: object }`; `options` defaults to `{}`.

`AgentCatalog` is `{ agents: AgentInfo[], defaultAgent: string|null }`. `AgentInfo` uses camelCase and contains required `name` and `mode`, optional `description`, `topP`, `temperature`, `color`, `model`, `variant`, `prompt`, and `steps`, booleans `native` and `hidden` defaulting to `false`, and objects `permission` and `options` defaulting to `{}`.

`CommandInfo` is `{ name, description, template, agent, model, subtask }`; it does not have a struct-level casing transform, so those exact lower-case field names apply and all fields default (`name` to `""`, the rest to `null`).

`SkillInfo` is `{ id, name, description, path }`; `id` and `name` are required, while `description` and `path` default to `null`.

`SystemContextSection` is `{ id, title, content }`; `title` is nullable. `PromptRequest` is `{ promptId, variables, service }`, with `variables` defaulting to `{}`. `RenderedPrompt` is `{ content, system }`, with `system` defaulting to `false`. `ConfigDocument` is `{ values, provenance }`, with `provenance` defaulting to `{}`.

### Providers and streams

`UserModel` is `{ providerId, modelId, connectionId?, variant? }`. `ProviderModelMetadata` is `{ api, authEnv, limit, cost, options, headers }`; `api`, `limit`, and `cost` are nullable, while arrays/maps default empty. The nested exact Rust DTO remains authoritative for provider API, cost, and limit fields.

`ProviderGenerationRequest` uses camelCase and contains `providerId`, `modelId`, optional `connectionId`, `tenantId`, `workspaceId`, `sessionId`, `variant`, `textVerbosity`, and `api`, plus `authEnv`, `messages`, `tools`, `options`, and `headers`. Empty `authEnv`, `tools`, `options`, and `headers` are omitted when serialized by the host.

Each provider stream item must deserialize as one of these exact externally tagged objects, using `type`: `start`; `start-step`; `text-start { id }`; `text-delta { id, delta }`; `text-end { id }`; `reasoning-start { id }`; `reasoning-delta { id, delta }`; `reasoning-end { id }`; `reasoning-metadata { id, metadata }`; `tool-input-start { id, name }`; `tool-input-delta { id, delta }`; `tool-input-end { id }`; `tool-call { id, name, input }`; `tool-result { id, output }`; `tool-error { id, message }`; `finish-step { finish, totalTokens?, inputTokens, outputTokens, reasoningTokens, cacheReadTokens, cacheWriteTokens }`; `finish` with the same usage fields; or `error { message }`.

The plugin must send exactly one `open` before any item, then zero or more `item` frames, then exactly one `end` or `error`. Duplicate opens, items before open, end before open, request-ID mismatch, overflow, or backpressure failure terminates that stream with an error. Dropping the host-side consumer sends `$/cancelStream`.

`ProviderRouteRequest` is `{ action, providerId, connectionId, tenantId, workspaceId, hosted, body }`. `action` is one of `list`, `configured`, `authMethods`, `authGet`, `authSet`, `authRemove`, `oAuthAuthorize`, `oAuthCallback`, `connectionsList`, `connectionsCreate`, `connectionsRename`, `connectionsDelete`, or `connectionsSetDefault`; these spellings follow Serde's `camelCase` transformation exactly.

Media `kind` is `"image"` or `"video"`. `dataBase64` uses standard Base64 and the decoded media has its own host limit listed below.

### HTTP and WebSocket routes

A route declaration is `{ descriptor, priority }`. `descriptor` is `{ id, method, path, scope, requestSchema?, responseSchema? }`, where `method` is exactly `"GET"`, `"POST"`, `"PUT"`, `"PATCH"`, or `"DELETE"`, and `scope` is `"workspace"` or `"session"`.

Process route paths are normalized under `/v2/plugins/<plugin-id>` if they do not already start with that canonical prefix. A descriptor ID must be nonempty; a path must begin with `/` and may not contain `..`; and the final path may not escape the canonical prefix. A session route from a global, user, or workspace plugin must contain `:session_id`; a session-scoped plugin may expose only session routes.

`RouteRequest` is `{ tenantId, hosted, workspaceId, workspace, sessionId, actor, generation?, path, query, headers, body }`. Nullable fields are serialized as JSON `null` unless marked optional; `hosted` defaults to `false`; `path` is a string map; `query` is a map of string arrays; and `headers` is a string map.

`RouteResponse` is `{ status: uint16, headers: object, body: JSON }`, with `headers` and `body` defaulting to `{}` and JSON `null` respectively when deserialized from omitted fields.

A process WebSocket message is one of `{ "kind": "text", "data": string }`, `{ "kind": "binary", "data": byte[] }`, `{ "kind": "ping", "data": byte[] }`, `{ "kind": "pong", "data": byte[] }`, or `{ "kind": "close" }`. This enum uses adjacent `kind`/`data` tagging; `close` has no `data`.

After `route.websocket.open` is accepted, outbound plugin-to-client messages are stream items. Client-to-plugin messages arrive through `route.websocket.message`. Receiving or emitting `close`, dropping either consumer, cancellation, or generation retirement ends the bridge.

### Message parts and MCP

A message-part declaration is `{ id: string, version: uint32, schema: JSON, fallbackTextField?: string }`. It registers a `part` contribution whose schema envelope is `{ version, schema, fallbackTextField }`; it does not install a renderer or callback by itself.

An MCP declaration is `{ id, transport, configSchema, authentication, prompts, resources, tools }`. `transport` defaults to `""`, `configSchema` to JSON `null`, `authentication` to `[]`, and the three booleans to `false`.

MCP declarations are metadata only. Executable MCP behavior must be represented through ordinary tool and route declarations; declaring `tools: true`, `resources: true`, or `prompts: true` does not create callbacks.

### Reverse host methods and capability mapping

Every reverse call must use the exact initialize owner. The params and result shapes are the same as the Lua host table documented above.

| Reverse method | Capability |
| --- | --- |
| `host.config.get` | `config-read` |
| `host.config.set` | `config-write` |
| `host.workspace.read`, `host.workspace.list` | `workspace-read` |
| `host.workspace.write` | `workspace-write` |
| `host.event.publish` | `event-publish` |
| `host.network.request` | `network` |
| `host.process.spawn`, `host.process.cancel` | `process-spawn` |
| `host.task.spawn`, `host.task.cancel` | `task-spawn` |
| `host.secret.use` | `secret-use` |
| `host.secret.read` | `secret-read` |
| `host.prompt.read` | `prompt-read` |
| `host.message.read` | `message-read` |
| `host.response.transform` | `response-transform` |
| `host.provider.call` | `provider-access` |
| `host.policy.call` | `policy-invoke` |

The host audits each reverse request with plugin ID, instance ID, revision, generation, scope, workspace/scope IDs, mapped capability, method, and allowed/denied outcome.

### Cancellation, failures, and stale generations

The ordinary configured call timeout defaults to 60,000 ms and is clamped to 1,000–600,000 ms. Host timeout or an Agent cancellation removes the pending call, sends `$/cancel`, and returns an error to the caller. Initialize always uses the separate 8-second timeout.

Only one terminal reply is consumed for a call ID. Unknown or late reply IDs are ignored. EOF immediately fails all pending calls with `plugin process exited`; malformed stdout objects and non-protocol lines are logged and skipped; stderr is logged separately.

On retirement the host marks the instance inactive, revokes capabilities, fails all streams, sends `shutdown`, closes stdin, waits up to 1.5 seconds, then kills and waits for the child. A reverse request from an inactive instance returns `plugin instance is retired`; a request whose owner differs in any field returns `plugin reverse request has the wrong owner`.

Package-hosted plugins use strict startup: spawn, initialize, or declaration failure rejects the candidate so the last-known-good generation remains. Legacy `serve`/`entry`/`npm` configuration uses non-strict startup and may publish a degraded plugin with no contributions.

### Process hard limits

| Limit | Exact value |
| --- | --- |
| stdin/stdout protocol frame | 1,048,576 bytes |
| one stderr log line | 65,536 bytes |
| pending host-to-plugin calls | 128 |
| queued reverse requests | 64 |
| reverse broker encoded input | 262,144 bytes |
| reverse broker encoded output | 524,288 bytes |
| active streams | 32 |
| queued items per stream | 64 |
| encoded JSON per stream item | 262,144 bytes |
| cumulative encoded item bytes per stream | 16,777,216 bytes |
| decoded media bytes | 786,432 bytes |
| initialize timeout | 8 s |
| graceful process shutdown | 1.5 s |
| configured callback timeout | default 60 s, clamped to 1–600 s |

The TypeScript runtime independently limits frames to 1,048,576 bytes, pending reverse host requests to 128, and concurrently dispatched host frames to 128.

## TypeScript with `@neoism/plugin`

`@neoism/plugin` exports the author types, `definePlugin(plugin)`, and `runPlugin(plugin)`. `definePlugin` is an identity helper that type-checks a `NeoismPlugin`; `runPlugin` owns the stdio loop until shutdown.

The public `NeoismPlugin` shape is complete in this table.

| Field | Type and behavior |
| --- | --- |
| `name`, `version` | Optional strings returned during initialize. |
| `tools` | `PluginTool[]`, where each tool has `id`, `description`, JSON Schema `parameters`, and `execute(input, ToolContext): ToolResult` &#124; `Promise<ToolResult>`. |
| `hooks` | `Record<string, HookHandler>`, where `HookHandler(context, value)` returns the replacement value synchronously or asynchronously. |
| `events` | Optional `{ namespaces?: string[], handler?(event: Event): void` &#124; `Promise<void> }`. |
| `services` | Optional `PluginServices` containing `agents`, `commands`, `skills`, `systemContext`, `prompts`, `config`, and `providers`. |
| `routes` | `PluginRoute[]`; each has `{ descriptor, priority?, handle(request, context) }`. |
| `websocketRoutes` | `PluginWebSocketRoute[]`; each has `{ descriptor, priority?, open(request, context) }`. |
| `messageParts` | `PluginMessagePart[]` with `id`, `version`, `schema`, and optional `fallbackTextField`. |
| `mcp` | `PluginMcpDeclaration[]` with `id`, optional `transport`, `configSchema`, `authentication`, `prompts`, `resources`, and `tools`. |
| `initialize` | Optional `(InitializeContext) => void` &#124; `Promise<void>`, called before declarations are returned. |

Each unary service is `ServiceDeclaration<handler>` with `id`, optional `priority`, and `handler`. Agent handlers return `{ agents, defaultAgent? }`; command and skill handlers return arrays; system-context handlers return `{ id, title?, content }[]`; prompt handlers return `{ content, system? }`; and config handlers return `{ values, provenance? }`. `ServiceRequest` is `{ workspaceId?, directory?, options? }`, while `PromptRequest` is `{ promptId, variables?, service }`.

`PluginProvider` has `descriptor: { id, ... }`, optional `priority`, required `stream(request, context)`, and optional `metadata(model, context)`, `auth(providerId, context)`, `route(request, context)`, and `media(request, context)`. `stream` returns an `AsyncIterable` directly or through a promise.

`PluginWebSocketSession` has required `outbound: AsyncIterable<PluginWebSocketMessage>` and optional `message(message)` and `close()` callbacks. `PluginWebSocketMessage` is exactly `{ kind: "text", data: string }`, `{ kind: "binary" | "ping" | "pong", data: number[] }`, or `{ kind: "close" }`.

`ToolResult` is `{ output: string | unknown, title?: string, metadata?: unknown }`. `ServiceHandlerContext` and `ProviderContext` contain `host` and `signal`; `ToolContext` adds `directory`, optional `sessionId`, and optional `client`. `HostRequestOptions` is `{ signal?: AbortSignal, timeoutMs?: number }`.

Install the package version that matches the repository release, then use ESM:

```sh
npm install @neoism/plugin
```

```ts
import { definePlugin, runPlugin } from "@neoism/plugin";

const plugin = definePlugin({
  name: "Todo tools",
  version: "1.0.0",

  async initialize(context) {
    const label = await context.host.config.get("plugins.todo.label");
    console.error("initialized", context.pluginId, context.instanceId, label);
  },

  tools: [{
    id: "todo_count",
    description: "Count TODO markers in a UTF-8 file",
    parameters: {
      type: "object",
      properties: { path: { type: "string" } },
      required: ["path"],
      additionalProperties: false,
    },
    async execute(input, context) {
      const { path } = input as { path: string };
      const bytes = await context.host.workspace.read(path, { signal: context.signal });
      const text = new TextDecoder().decode(bytes);
      const count = text.match(/TODO/g)?.length ?? 0;
      return { title: "TODO count", output: String(count), metadata: { count } };
    },
  }],

  hooks: {
    "chat.options"(_context, value) {
      return { ...(value as object), temperature: 0 };
    },
  },

  events: {
    namespaces: ["session."],
    handler(event) { console.error("event", event.type); },
  },

  services: {
    systemContext: [{
      id: "todo.context",
      priority: 10,
      handler: async (_request, context) => [{
        id: "todo",
        content: String(await context.host.config.get("todo.policy") ?? "Resolve TODOs explicitly."),
      }],
    }],
  },

  routes: [{
    descriptor: { id: "todo.health", method: "GET", path: "/health", scope: "workspace" },
    handle: () => ({ status: 200, headers: {}, body: { ok: true } }),
  }],

  messageParts: [{
    id: "todo.summary",
    version: 1,
    schema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
    fallbackTextField: "text",
  }],
});

await runPlugin(plugin);
```

For a process package, compile that source to a runnable ESM file and point the manifest at Node:

```json
{
  "id": "dev.example.todo",
  "name": "Todo tools",
  "version": "1.0.0",
  "agent": {
    "runtime": "process",
    "entrypoint": "",
    "command": ["node", "dist/plugin.js"],
    "capabilities": ["config-read", "workspace-read"],
    "scope": "workspace"
  }
}
```

The TypeScript callback model intentionally differs from raw Lua in a few ergonomic places: tool `execute` receives `(input, ToolContext)` rather than the whole wire call; hooks receive `(context, value)`; service handlers receive `(request, ServiceHandlerContext)`; provider streams are `AsyncIterable`; and WebSocket `open` returns a session with `outbound`, optional `message`, and optional `close`.

`ToolContext` contains `directory`, optional `sessionId`, `host`, `signal`, and an optional unauthenticated SDK HTTP `client` when `NEOISM_AGENT_SERVER_URL` exists. `InitializeContext` contains `pluginId`, `instanceId`, `directory`, `config`, `host`, and optional `client`. Do not treat `client` as privileged: the process does not inherit `NEOISM_AGENT_TOKEN`.

`PluginHostServices` provides `config.get/set`, `workspace.read/write/list`, `events.publish`, `network.call`, `process.call/cancel`, `tasks.call/cancel`, `secrets.use/read`, and `prompts/messages/responses/providers/policy.call`. Each call accepts optional `{ signal, timeoutMs }`. Broker `.call` methods unwrap `{ output }`; workspace reads/writes convert between byte arrays and `Uint8Array`.

Provider `stream(request, context)` returns an `AsyncIterable`; the runtime emits open/item/end and converts an iterator failure to stream error. `$/cancelStream` aborts its signal and invokes iterator return/close cleanup. Unary and route handlers receive an `AbortSignal` that `$/cancel` aborts.

## Native Agent plugins

The native API major is `2`. A native plugin implements `PluginFactory`, returns a `PluginDescriptor`, and creates one `PluginInstance`; immutable plugins may implement `PluginDefinition` and use the automatic `StaticPluginInstance` adapter.

`PluginDescriptor` contains a native `PluginManifest`, exact `PluginScope`, `required_capabilities`, and `plugin_api_major`. The host validates the API major, runtime scope, required grants, dependencies, route prefix policy, contribution identities, and conflicts before publication.

`PluginInstance.start()` is called at most once before contributions are read. `readiness()` must return `Ready` or `Degraded` for installation to continue. `contributions()` returns an immutable snapshot. `shutdown()` must be idempotent and cancellation-safe because hosts may bound and retry terminal cleanup.

Native contributions can provide tools, hooks, source/service implementations for agents, commands, skills, config, system context, prompts, and providers, plus HTTP/WebSocket routes and declaration metadata. The process adapter itself is implemented as an ordinary native factory, so contribution validation and generation publication are shared.

This is a trusted cooperative tier, not a third-party sandbox. Native code shares the Agent process, can block threads, ignore cancellation, access ambient process memory and credentials, or terminate the process. The API cannot forcibly stop malicious code or a future that never returns. A package manifest's `nativeApproved` flag does not dynamically load an Agent native library; production native factories are constructed by trusted host code linked into the process.

The reusable conformance fixture demonstrates installation, manifest publication, contribution ownership, tool execution, hook lifecycle, and structural absence when disabled. Native plugin authors should copy that public-API-only harness and also test capability denial, conflicting IDs, startup failure cleanup, and idempotent shutdown.

```sh
cargo test -p neoism-agent-plugin-api --test plugin_conformance
```

## Opaque remote resources

The workspace daemon's plugin-resource protocol is a separate language-neutral transport for remote editor/plugin adapters; it is not a `neoism-plugin/2` method and `@neoism/plugin` does not expose it directly. Agent brokers may return analogous opaque IDs, but authors must use only operations documented by the concrete broker rather than assuming this daemon protocol is installed.

Remote resource IDs are random connection capabilities beginning with `pr_`. They are opaque: never parse, persist, or transfer them between owners, sockets, workspaces, revisions, or generations. The wire never exposes a host path, file descriptor, process ID, PTY handle, or DAP transport handle.

The exact owner is `{ pluginId, revision }`. Allocation also carries `workspaceId`, nonzero `generation`, and trust `{ packageDigest, artifactDigest, abiVersion, capabilities, userScope }`. The daemon verifies an exact active host approval before allocating anything.

Resource kinds are `workspace`, `file`, `directory`, `watch`, `task`, `test`, `pty`, and `dap`. Targets are `{ "kind": "workspace_root" }`, `{ "kind": "child", "parent": resource, "name": string }`, or `{ "kind": "command", "program": string, "arguments": string[], "cwd": string|null }`.

Requests use a `type` discriminator and are exactly `allocate`, `invoke`, `cancel`, `close`, or `close_owner`. Replies are exactly `allocated`, `result`, `closed`, `owner_closed`, or `error`; errors contain `{ code, message }` and normal rejection currently uses code `resource_rejected`.

Allocation is socket-local and workspace-bound. Allocating a newer revision or generation for the same plugin retires and cancels old bindings. Invocation requires exact owner and generation equality; stale, cross-owner, cross-socket, and cross-generation handles are rejected. Socket disconnect drops the broker and cancels all live process and PTY resources.

Contained child names are 1–255 bytes, cannot be `.` or `..`, and cannot contain `/` or `\`. File `read` defaults to 1 MiB and clamps `maxBytes` to 1–4 MiB; file `write` is limited to 4 MiB. Process and PTY stdin calls are limited to 256 KiB. Status drains at most 1 MiB of buffered output per call; total retained process or PTY output is capped at 4 MiB.

Command programs must be nonempty host-resolved names without `/`, `\`, or `:`. Process commands allow at most 256 arguments and each argument is at most 64 KiB; PTY commands allow at most 256 arguments. CWD is resolved component-by-component beneath the workspace. PTY dimensions must be 1–1,000 columns and rows.

## Source of truth

- Shared package DTOs, capability strings, scopes, lifecycle, and trust: [`neoism-agent/crates/neoism-agent-plugin-api/src/package.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/package.rs) and [`context.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/context.rs).
- Package discovery, ID validation, revision hashing, and authorization: [`neoism-agent/crates/neoism-agent-server/src/plugin_package.rs`](../neoism-agent/crates/neoism-agent-server/src/plugin_package.rs).
- Capability grants and contained workspace access: [`neoism-agent/crates/neoism-agent-server/src/plugins/mod.rs`](../neoism-agent/crates/neoism-agent-server/src/plugins/mod.rs).
- Canonical lifecycle and native contracts: [`neoism-agent/crates/neoism-agent-plugin-api/src/lib.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/lib.rs), [`plugin.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/plugin.rs), and [`scoped_plugin_runtime.rs`](../neoism-agent/crates/neoism-agent-server/src/scoped_plugin_runtime.rs).
- Process wire DTOs and Serde casing: [`neoism-agent/crates/neoism-agent-plugin-api/src/process_v2.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/process_v2.rs).
- Process host, method dispatch, owner validation, limits, streams, and route adapters: [`neoism-agent/crates/neoism-agent-server/src/plugin_host_process.rs`](../neoism-agent/crates/neoism-agent-server/src/plugin_host_process.rs).
- Lua declarations, containment, callbacks, and budgets: [`neoism-agent/crates/neoism-agent-lua-runner/src/main.rs`](../neoism-agent/crates/neoism-agent-lua-runner/src/main.rs).
- Service and route DTOs: [`neoism-agent/crates/neoism-agent-plugin-api/src/services.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/services.rs) and [`route.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/route.rs).
- Provider request and stream-event DTOs: [`neoism-agent/crates/neoism-agent-core/src/provider.rs`](../neoism-agent/crates/neoism-agent-core/src/provider.rs), [`api.rs`](../neoism-agent/crates/neoism-agent-core/src/api.rs), and [`session.rs`](../neoism-agent/crates/neoism-agent-core/src/session.rs).
- TypeScript author API and runtime: [`neoism-agent/sdk/typescript/packages/plugin/src/index.ts`](../neoism-agent/sdk/typescript/packages/plugin/src/index.ts) and its [`package.json`](../neoism-agent/sdk/typescript/packages/plugin/package.json).
- Process wire fixtures: [`neoism-agent/crates/neoism-agent-plugin-api/tests/process_v2_wire.rs`](../neoism-agent/crates/neoism-agent-plugin-api/tests/process_v2_wire.rs) and [`neoism-agent/sdk/typescript/scripts/plugin-host.test.mjs`](../neoism-agent/sdk/typescript/scripts/plugin-host.test.mjs).
- Native conformance fixture: [`neoism-agent/crates/neoism-agent-plugin-api/tests/plugin_conformance.rs`](../neoism-agent/crates/neoism-agent-plugin-api/tests/plugin_conformance.rs).
- Opaque remote-resource wire and daemon implementation: [`neoism-protocol/src/plugin_resource.rs`](../neoism-protocol/src/plugin_resource.rs) and [`neoism-workspace-daemon/src/plugin_resources.rs`](../neoism-workspace-daemon/src/plugin_resources.rs).