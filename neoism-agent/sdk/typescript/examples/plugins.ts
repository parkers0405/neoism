import {
  createHttpClient,
  mcp,
  subagents,
  vcs,
  workflows,
} from "@neoism/sdk";

const directory = process.env.NEOISM_DIRECTORY ?? process.cwd();
const client = createHttpClient({
  baseUrl: process.env.NEOISM_AGENT_URL ?? "http://127.0.0.1:4096",
  ...(process.env.NEOISM_AGENT_TOKEN
    ? { token: process.env.NEOISM_AGENT_TOKEN }
    : {}),
});

const capabilities = await client.capabilities.list(directory);
console.log(capabilities.filter((capability) => capability.enabled));

const workflowClient = await client.plugins.tryUse(workflows, { directory });
if (workflowClient) {
  const catalog = await workflowClient.list(directory);
  const latest = catalog.workflows[0];
  if (latest?.writable && latest.revision) {
    await workflowClient.patch(latest.definition.id, { name: latest.definition.name }, {
      directory,
      revision: latest.revision,
    });
  }
}
if (workflowClient) console.log(await workflowClient.list(directory));

const vcsClient = await client.plugins.tryUse(vcs, { directory });
if (vcsClient) console.log(await vcsClient.status(directory));

const mcpClient = await client.plugins.tryUse(mcp, { directory });
if (mcpClient) console.log(await mcpClient.catalog(directory));

const session = await client.sessions.create({ directory, title: "SDK plugins example" });
const subagentClient = await client.plugins.tryUse(subagents, { directory });
if (subagentClient) console.log(await subagentClient.list(session.id));