import { createEmbeddedNeoism } from "@neoism/sdk-node";

const database = process.argv[2];
const directory = process.argv[3];
if (!database || !directory) throw new Error("Usage: embedded DATABASE WORKSPACE");
const host = await createEmbeddedNeoism({ database,
  ...(process.env.NEOISM_AGENT_TOKEN ? { token: process.env.NEOISM_AGENT_TOKEN } : {}) });
try {
  const session = await host.client.sessions.create({ directory, title: "Embedded TypeScript application" });
  console.log(session.id);
  // The full existing client is available as host.client, including generated
  // operations, model/tool prompts, artifacts, permissions, and event streams.
} finally {
  await host.close();
}