import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
require("./patch-solid-host.cjs")();
// Consume this checkout's SDK, including its generated API contract. Building
// the facade's references also builds the transport and optional plugin clients.
const child = Bun.spawn(["node", require.resolve("typescript/bin/tsc"), "-b",
  "../../neoism-agent/sdk/typescript/packages/sdk"], { stdout: "inherit", stderr: "inherit" });
if (await child.exited !== 0) process.exit(1);
