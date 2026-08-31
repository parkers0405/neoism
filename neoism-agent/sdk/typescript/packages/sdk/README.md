# @neoism/sdk

Official TypeScript SDK for the Neoism Agent API.

```sh
npm install @neoism/sdk
```

```ts
import { createHttpClient, subagents, workflows } from "@neoism/sdk";

const client = createHttpClient({
  baseUrl: "http://127.0.0.1:4096",
  token: process.env.NEOISM_AGENT_TOKEN,
});

const sessions = await client.sessions.list();
const capabilities = await client.capabilities.list();

if (subagents.supported(capabilities)) {
  const tasks = await subagents.client(client).list(sessions.items[0].id);
  console.log(tasks);
}

const workflowClient = await client.plugins.tryUse(workflows);
if (workflowClient) console.log(await workflowClient.list());
```

The SDK is ESM-only and includes TypeScript declarations. Its HTTP transport
uses the standard Fetch and Streams APIs available in modern Node.js and web
runtimes.

Optional Agent Server features are capability-gated. `agents`, `commands`,
`providers`, `skills`, `goals`, `lsp`, `mcp`, `pty`, `semanticSearch`,
`subagents`, `vcs`, and `workflows` are exported as typed plugin SDKs. Bind
them with `client.plugins.use()` when required or `tryUse()` when optional.

An event subscription scoped with `{ sessionId }` follows the complete session
family, including child sessions created by subagents. The main agent owns
subagent execution; clients observe child messages, tools, status, permissions,
questions, and completion through the same typed SSE stream.

See the [Neoism repository](https://github.com/parkers0405/neoism/tree/main/neoism-agent/sdk/typescript)
for the complete headless example and API documentation.