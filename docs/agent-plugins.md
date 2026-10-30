# Agent plugin API

The GPUIX branch supports native Rust Agent plugins and supervised process plugins.
Read [plugin authoring](plugins.md) for manifests, exact-revision approval, scopes,
and capability boundaries. The bundled editor and Agent Lua runtimes are removed.

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

Every reverse call must use the exact initialize owner. The params and result shapes are defined by the process host frame DTOs in `neoism-agent-plugin-api`.

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

The TypeScript callback model uses these conventions: tool `execute` receives `(input, ToolContext)` rather than the whole wire call; hooks receive `(context, value)`; service handlers receive `(request, ServiceHandlerContext)`; provider streams are `AsyncIterable`; and WebSocket `open` returns a session with `outbound`, optional `message`, and optional `close`.

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
bash scripts/blaze.sh test -p neoism-agent-plugin-api --test plugin_conformance
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
- Service and route DTOs: [`neoism-agent/crates/neoism-agent-plugin-api/src/services.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/services.rs) and [`route.rs`](../neoism-agent/crates/neoism-agent-plugin-api/src/route.rs).
- Provider request and stream-event DTOs: [`neoism-agent/crates/neoism-agent-core/src/provider.rs`](../neoism-agent/crates/neoism-agent-core/src/provider.rs), [`api.rs`](../neoism-agent/crates/neoism-agent-core/src/api.rs), and [`session.rs`](../neoism-agent/crates/neoism-agent-core/src/session.rs).
- TypeScript author API and runtime: [`neoism-agent/sdk/typescript/packages/plugin/src/index.ts`](../neoism-agent/sdk/typescript/packages/plugin/src/index.ts) and its [`package.json`](../neoism-agent/sdk/typescript/packages/plugin/package.json).
- Process wire fixtures: [`neoism-agent/crates/neoism-agent-plugin-api/tests/process_v2_wire.rs`](../neoism-agent/crates/neoism-agent-plugin-api/tests/process_v2_wire.rs) and [`neoism-agent/sdk/typescript/scripts/plugin-host.test.mjs`](../neoism-agent/sdk/typescript/scripts/plugin-host.test.mjs).
- Native conformance fixture: [`neoism-agent/crates/neoism-agent-plugin-api/tests/plugin_conformance.rs`](../neoism-agent/crates/neoism-agent-plugin-api/tests/plugin_conformance.rs).
- Opaque remote-resource wire and daemon implementation: [`neoism-protocol/src/plugin_resource.rs`](../neoism-protocol/src/plugin_resource.rs) and [`neoism-workspace-daemon/src/plugin_resources.rs`](../neoism-workspace-daemon/src/plugin_resources.rs).