# SDK

The `@neoism/sdk-*` packages are typed TypeScript clients for the Neoism Agent API. They are generated from the same OpenAPI contract the server serves, version-locked to the server release, and used by Neoism's own plugin tooling — the types cannot drift from the running server.

## Packages

| Package | Contents |
|---|---|
| `@neoism/sdk-core` | The generated contract (every operation and schema), a typed client façade, and the plugin-extension model. |
| `@neoism/sdk-http` | Fetch transport with SSE streaming, `Last-Event-ID` resume, sequence dedupe, and bounded reconnect backoff. |
| `@neoism/sdk-node` | Node re-exports of core + http. |
| `@neoism/plugin` | The serve-plugin author API (`definePlugin` / `runPlugin`) — see [[Plugins]]. |
| `@neoism/sdk-all` | Everything above in one dependency. |

## Use it

```ts
import { createHttpClient } from "@neoism/sdk-http";

const client = createHttpClient({
  baseUrl: "http://127.0.0.1:4096",
  token: process.env.NEOISM_AGENT_TOKEN,
});

// Curated façade calls, or the full generated surface by operation id:
const sessions = await client.operations.request("v2.sessions.list", {});
await client.operations.request("v2.sessions.prompt", {
  path: { session_id: sessions[0].id },
  body: { prompt: "summarize the last change", delivery: "queue" },
});
```

## Typed events

The event stream is a discriminated union — `switch` on `type` and the payload narrows:

```ts
for await (const event of client.events({ sessionId })) {
  switch (event.type) {
    case "message.part.delta":
      process.stdout.write(event.data.delta);
      break;
    case "session.status":
      console.log(event.data.status);
      break;
    case "permission.asked":
      // event.data is the full typed permission request
      break;
  }
}
```

Every published event type has a schema in the contract; an event the server can emit but the contract doesn't describe fails the server's own test suite.

## Typed parts

Message parts are the same kind of union, discriminated by `type` — text,
reasoning, tool, subtask, step-start, step-finish, compaction, agent, and
file. Tool parts carry a status-discriminated state machine, and
step-finish parts carry typed token usage and cost:

```ts
for (const part of message.parts) {
  if (part.type === "tool" && part.state.status === "completed") {
    console.log(part.tool, part.state.title);
  }
  if (part.type === "step-finish") {
    billing.record(part.tokens.input + part.tokens.output, part.cost);
  }
}
```

`message.part.updated` events carry the same typed `Part`, so live handling
and transcript reads share one set of narrowing branches.

## A complete embedding loop

`sdk/typescript/examples/headless.ts` in the repository is the full
headless driver — create a session in a directory, subscribe before
prompting, prompt with an idempotent `messageId`, stream typed deltas,
collect step-finish usage, detect idle, and read the transcript back. The
"Embedding the agent in a product" section of [[Server and API]] walks
through the same loop.

## How it stays in sync

- The OpenAPI document is authoritative and committed (`neoism-agent/openapi/v2.json`).
- Server tests enforce route/spec parity in both directions — including plugin-contributed routes — and event-union exhaustiveness.
- CI regenerates and byte-compares the TypeScript contract; a drifted contract fails the build.
- Releases publish the packages version-locked to the server version.

## Regenerate locally

After an intentional API change:

```sh
bash neoism-agent/scripts/openapi.sh update
```

This refreshes the committed spec, its fingerprint, and the generated contract in one step. `check` mode is what CI runs.

See [[Server and API]] for the endpoints behind the client and [[Plugins]] for building on the SDK inside a plugin.
