# Neoism TypeScript SDK

Frontend-neutral clients for the Neoism Agent `/v2` API.

This package contains two distinct surfaces: `@neoism/sdk` is the generated HTTP client, while `@neoism/plugin` authors supervised `neoism-plugin/2` processes. The complete package, Agent Lua, process-wire, native-plugin, trust, capability, scope, lifecycle, limit and remote-resource reference is [`docs/agent-plugins.md`](../../../docs/agent-plugins.md); shared package architecture is in [`docs/plugins.md`](../../../docs/plugins.md). Do not infer the process author API from generated OpenAPI client types.

```sh
npm install @neoism/sdk
```

```ts
import { createHttpClient, subagents } from "@neoism/sdk";

const client = createHttpClient({
  baseUrl: "http://127.0.0.1:4096",
  token: process.env.NEOISM_AGENT_TOKEN,
});
const capabilities = await client.capabilities.list();

if (subagents.supported(capabilities)) {
  const tasks = await subagents.client(client).list("session-id");
}

// Any plugin can be consumed without adding it to the SDK core.
const vcs = await client.plugins.request<unknown>("dev.neoism.vcs", "status", {
  query: { directory: "/workspace" },
});

for await (const event of client.transport.events()) {
  console.log(event.sequence, event.type, event.data);
}
```

Unknown events and plugin payloads remain `unknown` and are preserved. The HTTP
transport resumes with `Last-Event-ID`, deduplicates by durable event ID, and
reconnects with bounded exponential backoff.

## Optional capabilities

Agent Server features supplied by plugins remain optional at runtime even
though their types ship in the one SDK package. Bind them through capability
discovery instead of assuming they are enabled:

```ts
import { createHttpClient, subagents, vcs, workflows } from "@neoism/sdk";

const client = createHttpClient({ baseUrl: "http://127.0.0.1:4096" });
const workflowClient = await client.plugins.tryUse(workflows, {
  directory: "/workspace",
});

if (workflowClient) {
  const catalog = await workflowClient.list("/workspace");
  console.log(catalog.workflows);
}

// The main agent starts subagents. Clients observe their child sessions and
// may list or stop tasks, matching the Neoism GUI control surface.
const subagentClient = await client.plugins.tryUse(subagents);
```

`client.events.subscribe({ sessionId })` includes that session's descendants,
matching Neoism GUI's one-stream session-family model. The main agent starts
subagents; SDK clients observe their child-session events and may list or stop
tasks through the optional subagents client.

Hosted clients may omit `sessionId` to consume the authenticated tenant-wide stream. The server scopes replay and live delivery to the resolved tenant before emitting events. `limit` bounds replay without changing the reconnect cursor.

Tenant-owned sessions support revision-guarded actor control. A service account can start work and a human can take control of the same session without cloning its transcript or artifacts:

```ts
const current = await client.sessions.control(sessionId);
const lease = await client.sessions.claimControl(sessionId, {
  expectedRevision: current?.revision ?? 0,
  leaseSeconds: 60,
});
const participants = await client.sessions.participants(sessionId);
await client.sessions.releaseControl(sessionId, lease.revision);
```

The package exports capability-gated clients for agents, commands, providers,
skills, goals, LSP, MCP, PTY, semantic search, subagents, VCS, and workflows.
`client.operations` remains the complete generated protocol escape hatch.

Authenticated local management is exposed through `client.management`,
including `workspaces` and `repositories`. Repository creation accepts either
`{ kind: "existing", path }` or `{ kind: "clone", remoteUrl, ref?, depth? }`.
Deleting either registration never removes files from its working tree.
Management reads and writes require a local operator token; trusted loopback
runtime access and workspace-scoped daemon credentials do not grant management
authority. Hosted provider and MCP credentials are supplied by tenant-scoped host stores and are never read from the local desktop credential files.

Loopback servers may run in trusted mode without a token. Non-loopback
`neoism-agent serve` requires `NEOISM_AGENT_TOKEN` or
`NEOISM_AGENT_AUTH_CONFIG`; callers send a configured token as a Bearer token.
`NEOISM_AGENT_ALLOW_UNAUTHENTICATED_REMOTE=1` is an explicit unsafe
escape hatch for deployments that provide authentication in an upstream
gateway.

Shared hosted deployments embed `neoism-agent-server` and inject a `TenantResolver`, non-native `ExecutionProvider`, shared `ArtifactBlobStore`, and tenant-scoped provider and MCP credential stores. Call `for_hosted_control_plane()` when constructing services so the server refuses startup if any required boundary is missing. The resolver turns each short-lived Bearer token into the authoritative tenant, actor, scope, quota, workspace, and execution policy. See [Hosted control plane embedding](../../docs/hosted-control-plane.md).

Hosted claims scope sessions, event streams, interactions, artifacts, audit entries, credentials, runtime generations, and quotas. Directory strings and model arguments never establish tenant identity. Global local configuration and native process routes remain unavailable to hosted actors.

Set `NEOISM_AGENT_ARTIFACT_SCAN_COMMAND` to an executable that accepts the
temporary upload path and exits successfully only for accepted content. Scanner
execution is limited to 60 seconds and rejected uploads are deleted.

## Contract generation

`neoism-agent/openapi/v2.sha256` fingerprints the canonical semantic document. A
dependency-free generator produces
`packages/core/src/generated/contract.ts` from its schemas and operations.

```sh
cargo run -p neoism-agent -- openapi
# or, from this directory
npm run contract:print
npm run contract:check
```

Use `npm run contract:update` after an intentional API change. The Rust parity test checks every router method and path in both directions; `contract:check` then compares the OpenAPI snapshot semantically, checks its canonical fingerprint, and byte-compares generated TypeScript types.

## Process plugins

External hook plugins can declare `command` as an executable string or argument
array in their JSON manifest. Neoism invokes the command with one JSON request
on stdin and expects one JSON response on stdout. The protocol is
`neoism-plugin/1`; invocations have configurable bounded timeouts and a 4 MiB
response limit. This subprocess boundary avoids exposing an unstable Rust ABI.
On Linux, Neoism automatically uses Bubblewrap when available. Hosted mode
requires it: plugin files and system runtimes are read-only, `/tmp` is ephemeral,
and network access is disabled unless the plugin manifest sets `network: true`.
Set `sandbox: true` to require isolation locally or
`NEOISM_AGENT_PLUGIN_SANDBOX=off` to explicitly disable automatic local use.
Hook failures and timeouts mark plugin status unhealthy; a later successful
invocation restores health.
## Serve plugins (`neoism-plugin/2`) — the third-party plugin runtime

A serve plugin is a long-lived process the agent server spawns once per
workspace plugin generation. It declares tools, hooks, events, providers,
HTTP/WebSocket routes, message-part schemas, MCP metadata, and catalog services
subscriptions at handshake, and they register into the same runtime registry
native plugins use — tools run through the normal permission pipeline, hook
failures surface in `/v2/plugins`, and generation reloads restart the
process cleanly.

Author one with `@neoism/plugin`:

```ts
import { definePlugin, runPlugin } from "@neoism/plugin";

await runPlugin(definePlugin({
  tools: [{
    id: "todo_count",
    description: "Count TODO markers",
    parameters: { type: "object", properties: {} },
    async execute(_input, context) {
      const files = await context.host.workspace.list(".");
      return { output: `${files.length} workspace entries` };
    },
  }],
  hooks: { "chat.options": (_context, value) => ({ ...value, temperature: 0 }) },
  events: { namespaces: ["session."], handler: (event) => console.error(event.type) },
  services: {
    commands: [{
      id: "dev.example.commands",
      handler: () => [{ name: "todos", description: "List TODOs" }],
    }],
  },
}));
```

Legacy configuration-driven process plugins remain supported through the workspace `plugins` map using any one of:

```jsonc
"plugins": {
  "dev.example.local":   { "options": { "entry": "./plugins/todos" } },
  "dev.example.npm":     { "options": { "npm": "@example/neoism-plugin@1.0.0" } },
  "dev.example.custom":  { "options": { "serve": ["python3", "plugin.py"] } }
}
```

`npm:` packages install into the server's plugin cache in the background;
the plugin reports `Degraded ("installing …")` until the install lands, then
the next generation refresh brings it live. Options: `config` (passed to the
plugin's `initialize`), `env`, `timeoutMs` (per call), `network`, `sandbox`, and
an explicit `capabilities` array such as `["config-read", "workspace-write",
"event-publish"]`. The process always receives only the intersection of these
requested grants and host policy. A
plugin that fails to spawn or handshake degrades with a reason instead of
breaking the workspace.

New distributable packages should use the shared `neoism-plugin.json` `agent` target documented in [`docs/agent-plugins.md`](../../../docs/agent-plugins.md). Existing configuration-driven plugins do not need to migrate unless they want shared editor/Agent identity, immutable package revisions, exact-revision trust or the package lifecycle UI.

Any language works: speak newline-delimited JSON on stdio — reply to
`initialize` with `{ protocol: "neoism-plugin/2", tools, hooks,
eventNamespaces, services, routes, websocketRoutes, messageParts, mcp }`, answer `tool.invoke`, `hook.invoke`, and the
declared unary service methods by echoing the
request `id` with a `result` (or `error`), treat `event` frames as
notifications, and exit on `shutdown`.

Unary services include agent/command/skill list, system-context sections, prompt render, and config load. Providers use bounded open/item/end/error streams; HTTP and WebSocket handlers use the native route registry. All declarations become ordinary `PluginContributions` and are conflict-checked and retired by `PluginHost` exactly like native services. A plugin can call capability-scoped host methods (`host.config.*`, `host.workspace.*`, and `host.event.publish`) over the same stdio channel. Every reverse request and stream frame carries the exact package, instance, scope, workspace, and registry-generation owner supplied during initialization. Stale generations are ignored or rejected. Host resource paths remain relative and opaque, no agent-server bearer token is handed to the process, frames and queues are bounded, and cancellation sends `$/cancel` or `$/cancelStream`.

Shared packages use `neoism-plugin.json` and an explicit `agent` entrypoint. Workspace packages are discovered as metadata only and execute only when the installation-owned plugin configuration contains an exact workspace/package/revision/capability trust record plus runtime approval. Lua Agent entrypoints run in the dedicated `neoism-agent-lua-runner` subprocess, not in the server: system libraries and native loading are removed, `require` is package-contained, and memory/instruction/wall-time/frame/queue budgets are enforced. Trust is invalidated whenever any package file changes.

Capability brokerage remains host-owned. `context.host` exposes separate network, process, task, secret-use, explicit secret-read, prompt, message, response-transform, provider, and policy brokers. Calls carry the exact package generation and scope owner, use opaque resource/task identities, are input/output bounded, audited, cancellable where applicable, and stop working as soon as the generation lease is revoked. Declaring a capability does not grant it: the package declaration, installation-owned exact-revision trust record, and an installed host broker must all intersect. `secrets.use()` performs host-defined signing/authorization operations without revealing credential material; raw `secrets.read()` is a separate capability.

Agent entrypoints may declare `global`, `user`, `workspace`, or `session` scope. User/session activation requires an exact `scopeId` trust pin, workspace/session activation additionally binds the workspace identity, and workspace-located packages cannot claim installation scope. Lifecycle records exposed by the plugin API include requested versus granted capabilities, source, revision, scope, retained revision/lease state, and actionable diagnostics.
