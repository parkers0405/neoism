import { createHttpClient, workflows } from "@neoism/sdk";

const token = process.env.NEOISM_AGENT_TOKEN;
if (!token) throw new Error("NEOISM_AGENT_TOKEN is required for management API access");

const directory = process.env.NEOISM_DIRECTORY ?? process.cwd();
const client = createHttpClient({
  baseUrl: process.env.NEOISM_AGENT_URL ?? "http://127.0.0.1:4096",
  token,
});

console.log(await client.management.workspaces.list());
console.log(await client.management.repositories.list());
console.log(await client.management.agents.list({ directory }));
console.log(await client.management.commands.list({ directory }));
console.log(await client.management.skills.list({ directory }));

const workflowClient = await client.plugins.tryUse(workflows, { directory });
if (workflowClient) {
  const catalog = await workflowClient.list(directory);
  const workflow = catalog.workflows.find((candidate) => candidate.writable);
  if (workflow?.revision) {
    await workflowClient.patch(workflow.definition.id, { name: workflow.definition.name }, {
      directory,
      revision: workflow.revision,
    });
  }
}
