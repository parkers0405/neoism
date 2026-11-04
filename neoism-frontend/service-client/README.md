# @neoism/service-client

Shared TypeScript daemon transport and wire schemas for frontend hosts. This package does not discover config, start a daemon, or load a GUI renderer. Source exports are intended for Bun/TypeScript consumers.

```ts
import { DaemonConnection } from "@neoism/service-client";

const client = new DaemonConnection({
  endpoint: "ws://localhost:7980/session",
  clientName: "neoism-tui",
  // authToken: legacy WebSocket upgrade credential
  // pairingToken: paired-device Hello credential
});
const unsubscribe = client.subscribe(event => console.log(event.family));
client.start();
// On host shutdown:
unsubscribe();
client.stop();
```

The original `(config, socketFactory?, onFailure?)` constructor remains supported. `clientName` defaults to `neoism-gpuix` for compatibility. An optional fourth argument supplies `profileStart`, `profileEnd`, and an optional host `batch` function; shared profiling defaults to no-op. GPUIX's compatibility wrapper supplies its own profiling and Solid batching so installed renderer runtimes retain atomic subscriber updates.

`./client` exports the connection, Effect service/layer, config/hooks, and errors; `./protocol` exports the existing schemas, helpers, and authoritative wire types from `neoism-protocol/typescript/types`. Configuration discovery and process startup belong to each host.

Verification: `bun test ./tests` and `npm run typecheck` after installing dependencies.