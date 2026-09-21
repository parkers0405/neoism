# Neoism TypeScript SDK

Frontend-neutral clients for the Neoism Agent `/v2` API.

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
workspace plugin generation. It declares tools, hooks, and event
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
      // context.client is an SDK client bound to the local agent server.
      return { output: `workspace: ${context.directory}` };
    },
  }],
  hooks: { "chat.options": (_context, value) => ({ ...value, temperature: 0 }) },
  events: { namespaces: ["session."], handler: (event) => console.error(event.type) },
}));
```

Configure it in the workspace `plugins` map — any one of:

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
plugin's `initialize`), `env`, `timeoutMs` (per call), `network`
(default `true` — SDK callbacks need loopback), and `sandbox` as above. A
plugin that fails to spawn or handshake degrades with a reason instead of
breaking the workspace.

Any language works: speak newline-delimited JSON on stdio — reply to
`initialize` with `{ protocol: "neoism-plugin/2", tools, hooks,
eventNamespaces }`, answer `tool.invoke` / `hook.invoke` by echoing the
request `id` with a `result` (or `error`), treat `event` frames as
notifications, and exit on `shutdown`.
