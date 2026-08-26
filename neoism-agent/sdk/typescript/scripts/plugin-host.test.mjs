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
    execute: (input) => ({ output: String(input.text).toUpperCase(), title: "shouted" }),
  }],
  hooks: {
    "chat.options": (_context, value) => ({ ...value, fixture: true }),
  },
  events: { namespaces: ["session."] },
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
      const reply = replies.find((entry) => entry.id === id);
      if (reply) return resolveReply(reply);
      if (Date.now() > deadline) return reject(new Error(`timed out waiting for reply ${id}`));
      setTimeout(poll, 10);
    };
    poll();
  });

send({ id: 1, method: "initialize", params: { protocol: "neoism-plugin/2", pluginId: "dev.test", directory: "/tmp", config: {} } });
const initialized = await waitFor(1);
assert.equal(initialized.result.protocol, "neoism-plugin/2");
assert.equal(initialized.result.tools.length, 1);
assert.equal(initialized.result.tools[0].id, "shout");
assert.deepEqual(initialized.result.hooks, ["chat.options"]);
assert.deepEqual(initialized.result.eventNamespaces, ["session."]);

send({ id: 2, method: "tool.invoke", params: { tool: "shout", directory: "/tmp", input: { text: "hi" } } });
const tooled = await waitFor(2);
assert.equal(tooled.result.output, "HI");
assert.equal(tooled.result.title, "shouted");

send({ id: 3, method: "hook.invoke", params: { hook: "chat.options", context: {}, value: { keep: 1 } } });
const hooked = await waitFor(3);
assert.deepEqual(hooked.result, { keep: 1, fixture: true });

send({ id: 4, method: "tool.invoke", params: { tool: "missing", directory: "/tmp", input: {} } });
const failed = await waitFor(4);
assert.match(failed.error, /unknown tool/);

send({ id: null, method: "event", params: { type: "session.updated", data: {} } });
const exited = new Promise((resolveExit) => child.on("exit", resolveExit));
send({ id: 5, method: "shutdown", params: {} });
await waitFor(5);
await exited;

console.log("plugin host tests passed");
