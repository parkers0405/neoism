import assert from "node:assert/strict";
import { inspect } from "node:util";
import { CloudRuntimeClient, CloudRuntimeError, createHttpClient } from "../packages/http/dist/index.js";

const allocation = {
  owner: { tenant: "tenant:org.example", workspace: "workspace:workspace_1.dev" }, generation: 7, provider: "bridge",
  spec: { image: "worker_v1", region: "us_east", vcpus: 2, memory_mib: 2048, disk_gib: 20 },
};
const handle = { owner: allocation.owner, generation: 7, provider: "bridge", machine_id: "machine_1" };
const connection = { version: 1, handle, agent_api_base_url: "https://worker.example/api/", transport: "https" };
const status = { handle, state: "running", ready: true, connection, failure: null };
const options = { endpoint: "https://bridge.example/prefix", provider: "bridge", bearer: "HOST_SECRET", capabilities: { isolation: "virtual_machine", stop_start: true, cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true } };
const calls = [];
const cloud = new CloudRuntimeClient({ ...options, fetch: async (url, init) => {
  calls.push({ url: String(url), ...init });
  return Response.json({ version: 2, status }, { status: 202 });
} });
assert.deepEqual(await cloud.ensure(allocation), status);
for (const action of ["start", "inspect", "stop", "destroy"]) assert.deepEqual(await cloud[action](allocation, handle), status);
assert.deepEqual(calls.map(x => x.url), ["ensure", "start", "inspect", "stop", "destroy"].map(x => `https://bridge.example/prefix/v2/runtime/${x}`));
for (const [i, call] of calls.entries()) {
  assert.equal(call.method, "POST");
  assert.equal(call.redirect, "manual");
  assert.equal(call.credentials, "omit");
  assert.deepEqual(call.headers, { authorization: "Bearer HOST_SECRET", "content-type": "application/json", accept: "application/json" });
  assert.deepEqual(JSON.parse(call.body), { version: 2, allocation, handle: i === 0 ? null : handle });
}
assert.deepEqual(cloud.capabilities, { isolation: "virtual_machine", stop_start: true, cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true });
assert.deepEqual(await cloud.inspect(allocation), status);
assert.equal(calls.at(-1).url, "https://bridge.example/prefix/v2/runtime/inspect");
assert.equal(JSON.parse(calls.at(-1).body).handle, null);
assert(!inspect(cloud).includes("HOST_SECRET"));
assert(!JSON.stringify(cloud).includes("HOST_SECRET"));
assert(!JSON.stringify(status).includes("HOST_SECRET"));
for (const returned of [
  { handle, state: "stopped", ready: false },
  { handle, state: "destroyed", ready: false, connection: null, failure: null },
  { handle, state: "starting", ready: false, failure: null },
  { handle, state: "running", ready: false, connection },
  { handle, state: "failed", ready: false, failure: { code: "unavailable", retryable: true } },
]) {
  const client = new CloudRuntimeClient({ ...options, fetch: async () => Response.json({ version: 2, status: returned }) });
  assert.deepEqual(await client.inspect(allocation, handle), returned);
}
const mutableAllocation = structuredClone(allocation);
const mutableHandle = structuredClone(handle);
const snapshotClient = new CloudRuntimeClient({ ...options, fetch: async () => {
  mutableAllocation.generation = 8;
  mutableHandle.generation = 8;
  return Response.json({ version: 2, status });
} });
assert.deepEqual(await snapshotClient.inspect(mutableAllocation, mutableHandle), status);

function checkError(code, retryable = false) {
  return error => {
    assert(error instanceof CloudRuntimeError);
    assert.equal(error.code, code);
    assert.equal(error.retryable, retryable);
    assert(!inspect(error).includes("HOST_SECRET"));
    assert(!inspect(error).includes("bridge.example"));
    assert(!inspect(error).includes("PRIVATE_RESPONSE"));
    assert.equal(error.cause, undefined);
    return true;
  };
}
for (const [httpStatus, code, retryable] of [[401, "unauthorized", false], [403, "unauthorized", false], [404, "not_found", false], [409, "conflict", false], [412, "conflict", false], [408, "timeout", true], [429, "unavailable", true], [503, "unavailable", true], [504, "timeout", true], [302, "rejected", false]]) {
  let attempts = 0;
  const client = new CloudRuntimeClient({ ...options, fetch: async () => {
    attempts++;
    return new Response("PRIVATE_RESPONSE HOST_SECRET", { status: httpStatus });
  } });
  await assert.rejects(client.destroy(allocation, handle), checkError(code, retryable));
  assert.equal(attempts, 1, "even ambiguous mutations are never retried");
}
for (const [body, code] of [
  [{ version: 1, status }, "protocol"],
  [{ version: 2, status, secret: "HOST_SECRET" }, "protocol"],
  [{ version: 2, status: { ...status, handle: { ...handle, generation: 8 } } }, "identity"],
  [{ version: 2, status: { ...status, handle: { ...handle, owner: { ...handle.owner, tenant: "foreign" } } } }, "identity"],
  [{ version: 2, status: { ...status, connection: null } }, "protocol"],
  [{ version: 2, status: { ...status, state: "stopped" } }, "protocol"],
  [{ version: 2, status: { ...status, failure: { code: "rejected", retryable: false } } }, "protocol"],
  [{ version: 2, status: { ...status, connection: { ...connection, handle: { ...handle, machine_id: "foreign" } } } }, "identity"],
  [{ version: 2, status: { ...status, connection: { ...connection, agent_api_base_url: "https://HOST_SECRET@worker.example/" } } }, "protocol"],
  [{ version: 2, status: { ...status, connection: { ...connection, agent_api_base_url: "http://worker.example/", transport: "development_loopback_http" } } }, "protocol"],
]) {
  const client = new CloudRuntimeClient({ ...options, fetch: async () => Response.json(body) });
  await assert.rejects(client.ensure(allocation), checkError(code));
}
for (const response of [new Response("invalid HOST_SECRET"), new Response("x".repeat(65_537)), new Response("{}", { headers: { "content-length": "65537" } })]) {
  const client = new CloudRuntimeClient({ ...options, fetch: async () => response });
  await assert.rejects(client.ensure(allocation), checkError("protocol"));
}
const transportFailure = new CloudRuntimeClient({ ...options, fetch: async () => { throw new Error("https://HOST_SECRET@bridge.example PRIVATE_RESPONSE"); } });
await assert.rejects(transportFailure.ensure(allocation), checkError("transport", true));
const timeout = new CloudRuntimeClient({ ...options, timeoutMs: 1, fetch: async (_, init) => new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(new Error("HOST_SECRET")))) });
await assert.rejects(timeout.stop(allocation, handle), checkError("timeout", true));
// Bound the test too: a deadline regression must fail rather than hang the suite.
async function timeoutWithin(pending) {
  let timer;
  try {
    await assert.rejects(Promise.race([pending, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("SDK request failed to honor its deadline")), 1000);
    })]), checkError("timeout", true));
  } finally { clearTimeout(timer); }
}
let hangingFetchAttempts = 0;
const ignoresAbort = new CloudRuntimeClient({ ...options, timeoutMs: 5, fetch: async () => {
  hangingFetchAttempts++;
  return new Promise(() => {});
} });
await timeoutWithin(ignoresAbort.ensure(allocation));
assert.equal(hangingFetchAttempts, 1);
let streamCancelled = false;
const hangsReading = new CloudRuntimeClient({ ...options, timeoutMs: 5, fetch: async () => new Response(new ReadableStream({
  pull() { return new Promise(() => {}); },
  cancel() { streamCancelled = true; return new Promise(() => {}); },
})) });
await timeoutWithin(hangsReading.inspect(allocation, handle));
assert.equal(streamCancelled, true);
const hangsCancelling = new CloudRuntimeClient({ ...options, timeoutMs: 100, fetch: async () => new Response(new ReadableStream({
  start(controller) { controller.enqueue(new Uint8Array(65_537)); },
  cancel() { return new Promise(() => {}); },
})) });
await assert.rejects(hangsCancelling.ensure(allocation), checkError("protocol"));
const redirected = Response.json({ version: 2, status });
Object.defineProperty(redirected, "redirected", { value: true });
const followsRedirects = new CloudRuntimeClient({ ...options, fetch: async () => redirected });
await assert.rejects(followsRedirects.ensure(allocation), checkError("protocol"));
let invalidCalls = 0;
const invalidClient = new CloudRuntimeClient({ ...options, fetch: async () => { invalidCalls++; throw Error(); } });
await assert.rejects(invalidClient.ensure({ ...allocation, generation: Number.MAX_SAFE_INTEGER + 1 }), checkError("identity"));
await assert.rejects(invalidClient.stop(allocation, { ...handle, generation: 6 }), checkError("identity"));
for (const badOwner of [{ ...allocation.owner, tenant: "tenant/foreign" }, { ...allocation.owner, workspace: "workspace other" }]) {
  await assert.rejects(invalidClient.ensure({ ...allocation, owner: badOwner }), checkError("identity"));
}
for (const patch of [{ image: "image:bad" }, { region: "region.bad" }]) {
  await assert.rejects(invalidClient.ensure({ ...allocation, spec: { ...allocation.spec, ...patch } }), checkError("identity"));
}
await assert.rejects(invalidClient.ensure({ ...allocation, provider: "bridge:bad" }), checkError("identity"));
await assert.rejects(invalidClient.inspect(allocation, { ...handle, machine_id: "machine.bad" }), checkError("identity"));
assert.throws(() => new CloudRuntimeClient({ ...options, provider: "bridge.bad" }), checkError("rejected"));
assert.equal(invalidCalls, 0);
for (const patch of [{ endpoint: "http://bridge.example" }, { endpoint: "https://HOST_SECRET@bridge.example" }, { endpoint: "https://bridge.example/?token=HOST_SECRET" }, { bearer: "" }, { bearer: "HOST_SECRET\n" }]) assert.throws(() => new CloudRuntimeClient({ ...options, ...patch }), e => e instanceof CloudRuntimeError && !inspect(e).includes("HOST_SECRET"));
new CloudRuntimeClient({ ...options, endpoint: "http://127.0.0.1:1234/", allowPlaintext: true });
globalThis.window = {};
try { assert.throws(() => new CloudRuntimeClient(options), checkError("rejected")); } finally { delete globalThis.window; }
// Server-side edge hosts are legitimate even when they expose WorkerGlobalScope.
globalThis.WorkerGlobalScope = class WorkerGlobalScope {};
try {
  const edgeHost = new CloudRuntimeClient({ ...options, fetch: async () => Response.json({ version: 2, status }) });
  assert.deepEqual(await edgeHost.ensure(allocation), status);
} finally { delete globalThis.WorkerGlobalScope; }

console.log("cloud lifecycle fake-fetch tests passed");

// Agent auth and model-provider account credentials stay on agent transport only.
const agentCalls = [];
const worker = { version: 1, root: "/workspace", tenantId: allocation.owner.tenant, workspaceId: allocation.owner.workspace, runtimeId: "runtime_1", runtimeGeneration: 7, expiresAt: 999999 };
const agent = createHttpClient({ baseUrl: "https://agent.example", token: "AGENT_SECRET", fetch: async (url, init) => {
  agentCalls.push({ url: String(url), ...init });
  if (String(url).endsWith("/v2/runtime")) return Response.json({ deployment: "workspace-worker", executionAvailable: true, worker });
  if (String(url).includes("/v2/capabilities")) return Response.json([{ id: "neoism.pty", version: "1.0.0", enabled: true, disableable: false, source: "builtin" }]);
  return Response.json(true);
} });
await agent.catalog.providers.setAuth("model", { type: "api", key: "MODEL_ACCOUNT_SECRET" });
await cloud.ensure(allocation);
for (const call of calls) {
  assert(!JSON.stringify(call).includes("AGENT_SECRET"));
  assert(!JSON.stringify(call).includes("MODEL_ACCOUNT_SECRET"));
}
assert.equal(agentCalls[0].headers.authorization, "Bearer AGENT_SECRET");
assert(!("cloud" in agent));
console.log("cloud credential isolation tests passed");
const runtime = await agent.runtime.get();
assert.deepEqual(runtime, { deployment: "workspace-worker", executionAvailable: true, worker });
assert.equal(runtime.worker.root, "/workspace");
assert.equal(agentCalls[1].url, "https://agent.example/v2/runtime");
assert.equal(agentCalls[1].method, "GET");
assert.equal(await agent.capabilities.has("neoism.pty"), true);
assert.equal(await agent.capabilities.has("neoism.nonexistent"), false);
console.log("cloud SDK wire, fencing, redaction, credential isolation, and runtime tests passed");

// Public descriptors are immutable snapshots in VM namespaces, never local paths.
const launch = { version: 1, runtime_id: "worker_7", root: "/workspace", state_root: "/state", expires_at: Math.floor(Date.now() / 1000) + 3600, verification_key: "A".repeat(43) };
const launchedAllocation = { ...allocation, launch: structuredClone(launch) };
let sentLaunch;
const descriptorClient = new CloudRuntimeClient({ ...options, fetch: async (_, init) => {
  sentLaunch = JSON.parse(init.body).allocation.launch;
  launchedAllocation.launch.root = "/mutated";
  return Response.json({ version: 2, status });
} });
assert.deepEqual(await descriptorClient.ensure(launchedAllocation), status);
assert.deepEqual(sentLaunch, launch);
for (const patch of [
  { version: 2 }, { runtime_id: "worker.bad" }, { root: "/" }, { root: "/workspace/" },
  { root: "/workspace/../escape" }, { root: "/workspace//nested" }, { root: "/workspace/child." },
  { root: "c:/workspace" }, { state_root: "/workspace/state" }, { root: "/state/nested" },
  { root: "C:/workspace", state_root: "C:/WORKSPACE/state" },
  { verification_key: "A".repeat(42) + "B" }, { verification_key: "A".repeat(43) + "=" },
  { private_seed: "PRIVATE_SIGNER" }, { expires_at: 0 },
]) await assert.rejects(invalidClient.ensure({ ...allocation, launch: { ...launch, ...patch } }), e => e instanceof CloudRuntimeError);
assert.equal(invalidCalls, 0);
const windows = new CloudRuntimeClient({ ...options, fetch: async () => Response.json({ version: 2, status }) });
await windows.ensure({ ...allocation, launch: { ...launch, root: "C:/workspace", state_root: "D:/state" } });
const expiredAllocation = { ...allocation, launch: { ...launch, expires_at: 1 } };
await assert.rejects(windows.ensure(expiredAllocation), checkError("rejected"));
await assert.rejects(windows.start(expiredAllocation, handle), checkError("rejected"));
await windows.inspect(expiredAllocation);
await windows.inspect(expiredAllocation, handle);
await windows.stop(expiredAllocation, handle);
await windows.destroy(expiredAllocation, handle);
for (const key of ["stop_start", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace"]) assert.throws(() => new CloudRuntimeClient({ ...options, capabilities: { ...options.capabilities, [key]: false } }), checkError("rejected"));
const containerCaps = { ...options.capabilities, isolation: "container", disk_limit: false };
assert.throws(() => new CloudRuntimeClient({ ...options, capabilities: containerCaps }), checkError("rejected"));
assert.throws(() => new CloudRuntimeClient({ ...options, developmentContainers: true, capabilities: { ...containerCaps, disk_limit: true } }), checkError("rejected"));
const dev = new CloudRuntimeClient({ ...options, developmentContainers: true, capabilities: containerCaps });
assert.equal(dev.capabilities.disk_limit, false);
assert.equal(dev.capabilities.isolation, "container");
assert(Object.isFrozen(dev.capabilities));
console.log("runtime v2 public launch descriptors and VM capability admission tests passed");
