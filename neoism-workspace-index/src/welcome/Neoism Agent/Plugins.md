# Plugins

Neoism Agent is plugin-first: providers, tools, MCP, LSP, PTY, VCS, workflows, skills, and commands are all plugins registered into one runtime. The same extension points are open to you — from a five-line hook script to a full third-party plugin package installed from npm.

## How the runtime works

Plugins are installed per workspace into an immutable **generation**. Editing configuration builds a new generation; in-flight requests keep a lease on the old one until they finish, then it is retired and shut down cleanly. This means plugin changes apply live — no server restart — and a misbehaving plugin can be torn down without touching the rest.

`GET /v2/plugins` lists every plugin with its version, capabilities, and live health. A plugin whose hook fails shows `active: false` with the failure reason; a later success restores it.

Every plugin declares the capabilities it needs — process spawn, workspace read/write, network, event publishing — and the host grants only what is declared.

## Serve plugins — write your own

A serve plugin is a long-lived process the server spawns and speaks to over stdio (`neoism-plugin/2`, newline-delimited JSON). It declares tools, hooks, and event subscriptions at handshake, and they register exactly like native ones: tools run through the normal permission pipeline, and failures degrade only that plugin.

Author one in TypeScript with `@neoism/plugin`:

```ts
import { definePlugin, runPlugin } from "@neoism/plugin";

await runPlugin(definePlugin({
  tools: [{
    id: "todo_count",
    description: "Count TODO markers in the workspace",
    parameters: { type: "object", properties: {} },
    async execute(_input, context) {
      // context.client is an SDK client bound to this agent server.
      return { output: `workspace: ${context.directory}` };
    },
  }],
  hooks: {
    "chat.options": (_context, value) => ({ ...value, temperature: 0 }),
  },
  events: {
    namespaces: ["session."],
    handler: (event) => console.error("saw", event.type),
  },
}));
```

Register it in the `plugins` map of your configuration — any of three sources:

```jsonc
{
  "agent": {
    "plugins": {
      "dev.example.local":  { "options": { "entry": "./plugins/todos" } },
      "dev.example.npm":    { "options": { "npm": "@example/neoism-plugin@1.0.0" } },
      "dev.example.custom": { "options": { "serve": ["python3", "plugin.py"] } }
    }
  }
}
```

- `entry` runs a local Node package or file.
- `npm` installs the package into the server's plugin cache in the background; the plugin shows `Degraded ("installing …")` until the install lands, then goes live on the next refresh automatically.
- `serve` runs any explicit command — the protocol is plain JSON on stdio, so any language works.

Additional options: `config` (an object handed to the plugin's `initialize`), `env`, `timeoutMs` (per call), `network` (default `true`; SDK callbacks need loopback), and `sandbox`.

## Hooks

Serve plugins and declarative plugins can intercept these named hooks:

| Hook | Moment |
|---|---|
| `chat.messages` | Transform the message list sent to the provider. |
| `chat.options` / `chat.headers` | Adjust provider request options and headers. |
| `tool.definition` | Rewrite a tool's advertised definition. |
| `tool.before` / `tool.after` | Inspect or modify tool input and results. |
| `shell.env` | Inject environment for shell tools. |
| `event` | Receive published events (subscribe by namespace). |

## Declarative process plugins

For static header/option injection or one-shot hook scripts, a JSON manifest under `plugins/*.json` (or a `plugins` map entry with `command`) still works: static `chatHeaders`/`chatOptions`/`shellEnv` maps merge directly, and a `command` runs one bounded subprocess per hook invocation (`neoism-plugin/1`). Serve plugins supersede this for anything stateful.

## Sandboxing

On Linux, plugin processes run under Bubblewrap when available: read-only workspace and system paths, ephemeral `/tmp`, and no network unless declared. Hosted mode requires the sandbox. Set `"sandbox": true` to require it locally, `false` to opt a trusted plugin out, or `NEOISM_AGENT_PLUGIN_SANDBOX=off` to disable automatic use.

## Failure policy

A plugin that fails to spawn, handshake, or answer never takes the workspace down. It reports `Degraded` with a reason in `/v2/plugins`, contributes nothing until it recovers, and the next generation rebuild retries it.

See [[SDK]] for the client packages a plugin builds on, [[Server and API]] for the event stream plugins can subscribe to, and [[Permissions]] — plugin tools obey the same rules as native ones.
