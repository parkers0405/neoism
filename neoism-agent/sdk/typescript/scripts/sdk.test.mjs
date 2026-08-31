import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  CapabilityUnavailableError,
  createNeoismClient,
} from "../packages/core/dist/index.js";
import { createHttpTransport } from "../packages/http/dist/index.js";
import {
  agents,
  commands,
  goals,
  lsp,
  mcp,
  providers,
  pty,
  semanticSearch,
  skills,
  vcs,
  workflows,
} from "../packages/plugin-builtins/dist/index.js";
import { subagents } from "../packages/plugin-subagents/dist/index.js";

const sdkEntry = await readFile(new URL("../packages/sdk/dist/index.js", import.meta.url), "utf8");
assert.match(sdkEntry, /sdk-plugin-builtins/);
assert.match(sdkEntry, /sdk-plugin-subagents/);

assert.deepEqual(
  [agents, commands, providers, skills, goals, lsp, mcp, pty, semanticSearch, vcs, workflows]
    .map((plugin) => plugin.capability),
  [
    "neoism.agents",
    "neoism.commands",
    "neoism.providers",
    "neoism.skills",
    "neoism.goals",
    "neoism.lsp",
    "neoism.mcp",
    "neoism.pty",
    "neoism.semantic",
    "neoism.vcs",
    "neoism.workflows",
  ],
);

const managementRequests = [];
const managementClient = createNeoismClient({
  async request(request) {
    managementRequests.push(request);
    if (request.path.endsWith("/versions")) return [];
    return { id: "review", writable: true };
  },
  async *events() {},
});
await managementClient.management.agents.create("review", { content: "Review changes." }, { directory: "/workspace" });
await managementClient.management.commands.update("audit", { content: "Audit $ARGUMENTS" }, { expectedRevision: "sha256:old" });
await managementClient.management.skills.versions("review", "/workspace");
await managementClient.management.skills.restore("review", "sv_1", { expectedRevision: "sha256:current" });
await managementClient.management.workspaces.create({ root: "/workspace" });
await managementClient.management.repositories.create({ kind: "existing", path: "/workspace" });
await managementClient.management.workspaces.delete("workspace-1", "sha256:workspace");
assert.deepEqual(
  managementRequests.map((request) => [request.method, request.path, request.query, request.headers]),
  [
    ["POST", "/v2/management/agents/review", { directory: "/workspace" }, { "content-type": "application/json" }],
    ["PUT", "/v2/management/commands/audit", {}, { "If-Match": "sha256:old", "content-type": "application/json" }],
    ["GET", "/v2/management/skills/review/versions", { directory: "/workspace" }, undefined],
    ["POST", "/v2/management/skills/review/versions/sv_1/restore", { expectedRevision: "sha256:current" }, { "If-Match": "sha256:current" }],
    ["POST", "/v2/management/workspaces", undefined, { "content-type": "application/json" }],
    ["POST", "/v2/management/repositories", undefined, { "content-type": "application/json" }],
    ["DELETE", "/v2/management/workspaces/workspace-1", {}, { "If-Match": "sha256:workspace" }],
  ],
);

const requests = [];
const transport = {
  async request(request) {
    requests.push(request);
    if (request.path === "/v2/capabilities") {
      return [{
        id: "neoism.workflows",
        enabled: true,
        version: "1.2.0",
        disableable: true,
        source: "builtin",
      }];
    }
    if (request.path.endsWith("/runs")) return { runs: [] };
    if (request.path.includes("/v2/plugins/dev.neoism.workflows")) return {};
    throw new Error(`unexpected request: ${request.path}`);
  },
  async *events() {},
};

const client = createNeoismClient(transport);
const workflowClient = await client.plugins.use(workflows, {
  directory: "/workspace",
  minimumVersion: "1.0.0",
});
await workflowClient.history("nightly", { directory: "/workspace", limit: 5 });
await workflowClient.patch("nightly", { name: "Nightly v2" }, { directory: "/workspace", revision: "sha256:old" });
await workflowClient.getRun("nightly", "run-1", "/workspace");
await workflowClient.retryRun("nightly", "run-1", "/workspace");
assert.deepEqual(requests[0].query, { directory: "/workspace" });
assert.equal(requests[1].path, "/v2/plugins/dev.neoism.workflows/nightly/runs");
assert.deepEqual(requests[1].query, { directory: "/workspace", limit: 5 });
assert.deepEqual(
  requests.slice(2).map((request) => [request.method, request.path, request.headers?.["If-Match"]]),
  [
    ["PATCH", "/v2/plugins/dev.neoism.workflows/nightly", "sha256:old"],
    ["GET", "/v2/plugins/dev.neoism.workflows/nightly/runs/run-1", undefined],
    ["POST", "/v2/plugins/dev.neoism.workflows/nightly/runs/run-1/retry", undefined],
  ],
);
assert.equal(subagents.capability, "neoism.subagents");

const builtinRequests = [];
const builtinClient = createNeoismClient({
  async request(request) { builtinRequests.push(request); return {}; },
  async *events() {},
});
await goals.client(builtinClient).get("session-1");
await semanticSearch.client(builtinClient).search("needle", { limit: 3 });
await vcs.client(builtinClient).rawDiff("/workspace");
await mcp.client(builtinClient).completeAuth("github", "code", { state: "state" });
await lsp.client(builtinClient).hover({ file: "src/main.rs", line: 4, character: 2 });
assert.deepEqual(
  builtinRequests.map((request) => [request.method, request.path, request.response]),
  [
    ["GET", "/v2/plugins/dev.neoism.goals/session-1", "json"],
    ["GET", "/v2/plugins/dev.neoism.semantic/search", "json"],
    ["GET", "/v2/plugins/dev.neoism.vcs/diff/raw", "text"],
    ["GET", "/v2/plugins/dev.neoism.mcp/github/auth/callback", "text"],
    ["GET", "/v2/plugins/dev.neoism.lsp/hover", "json"],
  ],
);

const absent = await client.plugins.tryUse(pty);
assert.equal(absent, undefined);
await assert.rejects(
  client.plugins.use(pty),
  (error) => error instanceof CapabilityUnavailableError,
);

const event = {
  id: "evt-1",
  sequence: 1,
  schemaVersion: "1",
  source: "test",
  timestamp: 1,
  type: "session.status",
  data: { sessionID: "session-1", status: { type: "idle" } },
};
const bytes = new TextEncoder().encode(`data: ${JSON.stringify(event)}\r\n\r\n`);
const abort = new AbortController();
const sse = createHttpTransport({
  baseUrl: "http://agent.test",
  fetch: async () => new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(bytes.subarray(0, 17));
      controller.enqueue(bytes.subarray(17));
      controller.close();
    },
  }), { status: 200, headers: { "content-type": "text/event-stream" } }),
});
const iterator = sse.events({ signal: abort.signal })[Symbol.asyncIterator]();
assert.deepEqual((await iterator.next()).value, event);
abort.abort();
await iterator.return?.();

let socketUrl;
class FakeWebSocket extends EventTarget {
  binaryType = "blob";
  sent = [];
  constructor(url) {
    super();
    socketUrl = url;
    queueMicrotask(() => this.dispatchEvent(new Event("open")));
  }
  send(data) { this.sent.push(data); }
  close() { this.dispatchEvent(new Event("close")); }
}
const websocketTransport = createHttpTransport({
  baseUrl: "https://agent.test/base",
  webSocket: (url) => new FakeWebSocket(url),
});
const websocket = await websocketTransport.connectSocket({
  path: "/v2/plugins/dev.neoism.pty/pty-1/connect",
  query: { ticket: "one use", cursor: 9 },
});
assert.equal(
  socketUrl,
  "wss://agent.test/v2/plugins/dev.neoism.pty/pty-1/connect?ticket=one+use&cursor=9",
);
websocket.close();

const ptyRequests = [];
const socketMessages = [
  "hello",
  Uint8Array.from([0, ...new TextEncoder().encode('{"cursor":12}')]),
];
const ptyTransport = {
  async request(request) {
    ptyRequests.push(request);
    if (request.path.endsWith("/connect-token")) return { ticket: "once", expires_in: 30 };
    if (request.method === "PUT") return { id: "pty-1", title: "Terminal" };
    throw new Error(`unexpected PTY request: ${request.path}`);
  },
  async *events() {},
  async connectSocket(request) {
    ptyRequests.push(request);
    return {
      send() {},
      close() {},
      async *messages() { yield* socketMessages; },
    };
  },
};
const ptyConnection = await pty.client(createNeoismClient(ptyTransport)).connect("pty/1", { cursor: 4 });
const output = [];
for await (const item of ptyConnection.output()) output.push(item);
assert.deepEqual(output, [
  { type: "data", data: "hello" },
  { type: "cursor", cursor: 12 },
]);
assert.equal(ptyRequests[0].headers["X-OpenCode-Ticket"], "1");
assert.equal(ptyRequests[1].path, "/v2/plugins/dev.neoism.pty/pty%2F1/connect");
assert.deepEqual(ptyRequests[1].query, { ticket: "once", cursor: 4 });

console.log("sdk consumer tests passed");