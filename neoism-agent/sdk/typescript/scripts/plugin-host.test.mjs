#!/usr/bin/env node
// End-to-end check of @neoism/plugin's stdio host loop: spawn a real plugin
// built from the package and drive the neoism-plugin/2 protocol against it.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const entry = resolve(here, "../packages/plugin/dist/index.js");

const program = `
import { definePlugin, runPlugin } from ${JSON.stringify(`file://${entry}`)};
await runPlugin(definePlugin({
  name: "fixture",
  tools: [{
    id: "shout",
    description: "uppercase",
    parameters: { type: "object" },
    execute: async (input, context) => ({ output: String(await context.host.config.get("prefix")) + String(input.text).toUpperCase(), title: "shouted" }),
  }, {
    id: "authorize",
    description: "use an opaque credential",
    parameters: { type: "object" },
    execute: async (_input, context) => ({ output: await context.host.secrets.use("sign", { resource: "opaque:request" }) }),
  }],
  hooks: {
    "chat.options": (_context, value) => ({ ...value, fixture: true }),
  },
  events: { namespaces: ["session."] },
  services: {
    commands: [{ id: "fixture.commands", priority: 5, handler: () => [{ name: "fixture" }] }],
    systemContext: [{ id: "fixture.context", handler: () => [{ id: "fixture", content: "context" }] }],
    providers: [{
      descriptor: { id: "fixture.provider", name: "Fixture" },
      async *stream(request) { yield { type: "text", text: request.prompt }; yield { type: "done" }; },
      metadata: (model) => ({ model }),
    }],
  },
  routes: [{ descriptor: { id: "fixture.route", method: "POST", path: "/echo" }, handle: (request) => ({ status: 200, body: request.body }) }],
  messageParts: [{ id: "fixture.part", version: 1, schema: { type: "object" }, fallbackTextField: "text" }],
  mcp: [{ id: "fixture.mcp", transport: "http", tools: true }],
}));
`;

const child = spawn(process.execPath, ["--input-type=module", "-e", program], {
  stdio: ["pipe", "pipe", "inherit"],
});

const replies = [];
let buffered = "";
child.stdout.on("data", (chunk) => {
  buffered += chunk.toString();
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    if (line.trim()) replies.push(JSON.parse(line));
  }
});

const send = (frame) => child.stdin.write(`${JSON.stringify(frame)}\n`);
const waitFor = (id) =>
  new Promise((resolveReply, reject) => {
    const deadline = Date.now() + 5000;
    const poll = () => {
      const reply = replies.find((entry) => entry.id === id && entry.method === undefined);
      if (reply) return resolveReply(reply);
      if (Date.now() > deadline) return reject(new Error(`timed out waiting for reply ${id}`));
      setTimeout(poll, 10);
    };
    poll();
  });
const waitForRequest = (method) =>
  new Promise((resolveRequest, reject) => {
    const deadline = Date.now() + 5000;
    const poll = () => {
      const request = replies.find((entry) => entry.method === method);
      if (request) return resolveRequest(request);
      if (Date.now() > deadline) return reject(new Error(`timed out waiting for request ${method}`));
      setTimeout(poll, 10);
    };
    poll();
  });
const waitForStream = (streamId, kind) =>
  new Promise((resolveFrame, reject) => {
    const deadline = Date.now() + 5000;
    const poll = () => {
      const frame = replies.find((entry) => entry.stream?.streamId === streamId && entry.stream?.kind === kind);
      if (frame) return resolveFrame(frame);
      if (Date.now() > deadline) return reject(new Error(`timed out waiting for ${kind} on ${streamId}`));
      setTimeout(poll, 10);
    };
    poll();
  });

const exactOwner = { pluginId: "dev.test", instanceId: "instance-9", packageRevision: "sha256:test", registryGeneration: 9, scope: "workspace", workspaceId: "workspace-1" };
send({ id: 1, method: "initialize", params: { protocol: "neoism-plugin/2", pluginId: "dev.test", instanceId: "instance-9", directory: "/tmp", config: {}, owner: exactOwner } });
const initialized = await waitFor(1);
assert.equal(initialized.result.protocol, "neoism-plugin/2");
assert.equal(initialized.result.tools.length, 2);
assert.equal(initialized.result.tools[0].id, "shout");
assert.deepEqual(initialized.result.hooks, ["chat.options"]);
assert.deepEqual(initialized.result.eventNamespaces, ["session."]);
assert.deepEqual(initialized.result.services.commands, [{ id: "fixture.commands", priority: 5 }]);
assert.equal(initialized.result.services.providers[0].descriptor.id, "fixture.provider");
assert.equal(initialized.result.routes[0].descriptor.id, "fixture.route");
assert.equal(initialized.result.messageParts[0].id, "fixture.part");
assert.equal(initialized.result.mcp[0].id, "fixture.mcp");

send({ id: 10, method: "tool.invoke", params: { tool: "shout", directory: "/tmp", input: { text: "hi" } } });
const reverse = await waitForRequest("host.config.get");
assert.deepEqual(reverse.owner, exactOwner);
assert.deepEqual(reverse.params, { key: "prefix" });
send({ id: reverse.id, result: ">" });
const tooled = await waitFor(10);
assert.equal(tooled.result.output, ">HI");
assert.equal(tooled.result.title, "shouted");

send({ id: 14, method: "tool.invoke", params: { tool: "authorize", directory: "/tmp", input: {} } });
const secretUse = await waitForRequest("host.secret.use");
assert.deepEqual(secretUse.owner, exactOwner);
assert.deepEqual(secretUse.params, { operation: "sign", input: { resource: "opaque:request" } });
send({ id: secretUse.id, result: { output: { authorizationHandle: "opaque:auth" } } });
assert.deepEqual(JSON.parse((await waitFor(14)).result.output), { authorizationHandle: "opaque:auth" });

send({ id: 11, method: "command.list", params: { serviceId: "fixture.commands", request: {} } });
assert.deepEqual((await waitFor(11)).result, [{ name: "fixture" }]);

send({ id: 3, method: "hook.invoke", params: { hook: "chat.options", context: {}, value: { keep: 1 } } });
const hooked = await waitFor(3);
assert.deepEqual(hooked.result, { keep: 1, fixture: true });

send({ id: 12, method: "provider.stream", params: { serviceId: "fixture.provider", streamId: "stream-2", requestId: 22, request: { prompt: "hello" } } });
await waitFor(12);
const streamOpen = await waitForStream("stream-2", "open");
assert.equal(streamOpen.requestId, 22);
assert.deepEqual(streamOpen.owner, exactOwner);
assert.deepEqual((await waitForStream("stream-2", "item")).stream.value, { type: "text", text: "hello" });
await waitForStream("stream-2", "end");

send({ id: 13, method: "route.handle", params: { routeId: "fixture.route", request: { body: "echo" } } });
assert.deepEqual((await waitFor(13)).result, { status: 200, body: "echo" });

send({ id: 4, method: "tool.invoke", params: { tool: "missing", directory: "/tmp", input: {} } });
const failed = await waitFor(4);
assert.match(failed.error, /unknown tool/);

send({ id: null, method: "event", params: { type: "session.updated", data: {} } });
const exited = new Promise((resolveExit) => child.on("exit", resolveExit));
send({ id: 5, method: "shutdown", params: {} });
await waitFor(5);
await exited;

console.log("plugin host tests passed");
