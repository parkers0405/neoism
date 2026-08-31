# @neoism/sdk

Official TypeScript SDK for the Neoism Agent API.

```sh
npm install @neoism/sdk
```

```ts
import { createHttpClient, subagents } from "@neoism/sdk";

const client = createHttpClient({
  baseUrl: "http://127.0.0.1:4096",
  token: process.env.NEOISM_AGENT_TOKEN,
});

const sessions = await client.sessions.list();
const capabilities = await client.capabilities.list();

if (subagents.supported(capabilities)) {
  const tasks = await subagents.client(client).list(sessions[0].id);
  console.log(tasks);
}
```

The SDK is ESM-only and includes TypeScript declarations. Its HTTP transport
uses the standard Fetch and Streams APIs available in modern Node.js and web
runtimes.

See the [Neoism repository](https://github.com/parkers0405/neoism/tree/main/neoism-agent/sdk/typescript)
for the complete headless example and API documentation.