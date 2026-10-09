import assert from "node:assert/strict";
import { inspect } from "node:util";
import { HostWorkspaceClient, connectWorkspaceWorker, CloudRuntimeError, createHttpTransport } from "../packages/http/dist/index.js";

const workspace = "workspace:one";
const tenant = "tenant:app";
const now = Math.floor(Date.now() / 1000);
const caps = { isolation: "virtual_machine", cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true, stop_start: true };
const handle = { owner: { tenant, workspace }, generation: 3, provider: "neutral", machine_id: "vm_3" };
const launch = { version: 1, runtime_id: "worker_3", root: "/workspace", state_root: "/state", expires_at: now + 3600, verification_key: "A".repeat(43) };
const allocation = { owner: handle.owner, generation: 3, provider: "neutral", spec: { image: "worker", region: "east", vcpus: 2, memory_mib: 2048, disk_gib: 20 }, launch };
const connection = { version: 1, handle, agent_api_base_url: "https://worker.test/api/", transport: "https" };
const status = { binding: { allocation, revision: 4, retired: false, pending: null, last_error: null, status: { handle, state: "running", ready: true, connection, failure: null } }, verification: null };
const grant = { version: 1, baseUrl: connection.agent_api_base_url, handle, root: launch.root, runtimeId: launch.runtime_id, workerGeneration: 3, bearer: "WORKER_ONLY", expiresAt: now + 300, capabilities: caps };
const runtime = { deployment: "workspace-worker", executionAvailable: true, worker: { version: 1, root: launch.root, tenantId: tenant, workspaceId: workspace, runtimeId: launch.runtime_id, runtimeGeneration: 3, expiresAt: launch.expires_at } };
const baseOptions = { endpoint: "https://host.test/gateway/", token: async () => "APP_AUTH_ONLY", trustedWorkerOrigins: ["https://worker.test"] };
let passed = 0;
async function check(name, run) { await run(); passed++; }
const rejected = error => {
  assert(error instanceof CloudRuntimeError);
  assert(!inspect(error).includes("WORKER_ONLY")); assert(!inspect(error).includes("APP_AUTH_ONLY"));
  assert(!inspect(error).includes("PRIVATE_SIGNER")); assert.equal(error.cause, undefined);
  return true;
};
async function within(pending) {
  let timer;
  try { return await Promise.race([pending, new Promise((_, reject) => { timer = setTimeout(() => reject(Error("Unbounded operation")), 1000); })]); }
  finally { clearTimeout(timer); }
}
function harness({ statusValue = status, grantValue = grant, runtimeValue = runtime, hostFetch, workerFetch } = {}) {
  const hostCalls = [], workerCalls = [];
  const host = new HostWorkspaceClient({ ...baseOptions, fetch: async (url, init) => {
    hostCalls.push({ url: String(url), ...init });
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer APP_AUTH_ONLY");
    assert.equal(init.redirect, "manual"); assert.equal(init.credentials, "omit");
    assert(!JSON.stringify(init).includes("WORKER_ONLY"));
    if (hostFetch) return hostFetch(url, init);
    return Response.json(String(url).endsWith("/connection") ? grantValue : statusValue);
  } });
  const fetch = async (url, init) => {
    workerCalls.push({ url: String(url), ...init });
    assert.equal(new URL(url).origin, "https://worker.test");
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer WORKER_ONLY");
    assert.equal(init.redirect, "manual"); assert.equal(init.credentials, "omit");
    assert(!JSON.stringify(init).includes("APP_AUTH_ONLY"));
    if (workerFetch) return workerFetch(url, init);
    return Response.json(String(url).endsWith("/v2/runtime") ? runtimeValue : []);
  };
  return { host, fetch, hostCalls, workerCalls, connect: () => connectWorkspaceWorker({ host, workspace, fetch }) };
}

await check("host policy owns tenant, spec and actor scopes, lifecycle is fenced", async () => {
  const h = harness();
  assert.deepEqual(await h.host.ensure(workspace), status);
  assert.deepEqual(await h.host.status(workspace), status);
  for (const action of ["start", "stop", "destroy"]) assert.deepEqual(await h.host[action](workspace, handle), status);
  assert.deepEqual(await h.host.connection(workspace), grant);
  assert.deepEqual(h.hostCalls.map(c => JSON.parse(c.body ?? "null")), [{}, null, { expected_handle: handle }, { expected_handle: handle }, { expected_handle: handle }, {}]);
  assert(h.hostCalls.every(c => !c.body || !Object.hasOwn(JSON.parse(c.body), "tenant")));
  assert(h.hostCalls.every(c => c.url.startsWith("https://host.test/gateway/v1/workspaces/workspace%3Aone/runtime/")));
  assert(!inspect(h.host).includes("APP_AUTH_ONLY")); assert(!JSON.stringify(h.host).includes("APP_AUTH_ONLY"));
});

await check("broker to normal typed worker client with cache and singleflight refresh", async () => {
  const h = harness();
  const client = await h.connect();
  assert.deepEqual(await client.runtime.get(), runtime);
  await Promise.all(Array.from({ length: 20 }, () => client.capabilities.list()));
  assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 1);
  assert.equal(h.hostCalls.filter(c => c.url.endsWith("/status")).length, 1);
  assert.equal(h.workerCalls[0].url, "https://worker.test/api/v2/runtime");
  assert(!JSON.stringify(client).includes("WORKER_ONLY")); assert(!inspect(client).includes("WORKER_ONLY"));
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    await Promise.all(Array.from({ length: 20 }, () => client.capabilities.list()));
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 2);
    await client.capabilities.list();
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 2, "short remaining lifetime must not cause a refresh storm");
  } finally { Date.now = originalNow; }
});

await check("malformed or foreign grants never reach a worker", async () => {
  for (const patch of [
    { version: 2 }, { handle: { ...handle, owner: { ...handle.owner, workspace: "other" } } },
    { handle: { ...handle, owner: { ...handle.owner, tenant: "other" } } },
    { workerGeneration: 4 }, { handle: { ...handle, generation: 4 }, workerGeneration: 4 },
    { expiresAt: now - 1 }, { expiresAt: launch.expires_at + 1 }, { root: "/foreign" },
    { runtimeId: "foreign" }, { baseUrl: "https://foreign.test/" },
    { baseUrl: "https://WORKER_ONLY@worker.test/api/" }, { baseUrl: "https://worker.test/api/?token=WORKER_ONLY" },
    { baseUrl: "https://worker.test/different/" }, { capabilities: { ...caps, disk_limit: false } },
    { privateSigner: "PRIVATE_SIGNER" }, { bearer: "bad\ncredential" },
  ]) {
    const h = harness({ grantValue: { ...grant, ...patch } });
    await assert.rejects(h.connect(), rejected); assert.equal(h.workerCalls.length, 0);
  }
});

await check("unready, pending, retired, expired or foreign status cannot issue a connection", async () => {
  for (const binding of [
    { ...status.binding, pending: "ensure" }, { ...status.binding, retired: true },
    { ...status.binding, status: { ...status.binding.status, state: "stopped", ready: false, connection: null } },
    { ...status.binding, allocation: { ...allocation, launch: { ...launch, expires_at: now - 1 } } },
    { ...status.binding, allocation: { ...allocation, owner: { ...allocation.owner, workspace: "other" } } },
    { ...status.binding, allocation: { ...allocation, launch: { ...launch, root: "/workspace/.." } } },
  ]) {
    const h = harness({ statusValue: { ...status, binding } });
    await assert.rejects(h.connect(), rejected);
    assert.equal(h.workerCalls.length, 0); assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 0);
  }
});

await check("running candidate connects only after authenticated broker verification", async () => {
  const candidate = { ...status, binding: { ...status.binding, status: { ...status.binding.status, ready: false } } };
  const h = harness({ statusValue: candidate });
  const client = await h.connect();
  assert.deepEqual(await client.runtime.get(), runtime);
  assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 1);
  const denied = harness({ hostFetch: async url => String(url).endsWith("/connection")
    ? Response.json({ code: "unready" }, { status: 503 }) : Response.json(candidate) });
  await assert.rejects(denied.connect(), rejected);
  assert.equal(denied.workerCalls.length, 0);
});

await check("worker runtime identity and expired lease are checked on connect", async () => {
  for (const worker of [
    { ...runtime.worker, version: 2 }, { ...runtime.worker, root: "/foreign" }, { ...runtime.worker, secret: "PRIVATE_SIGNER" },
    { ...runtime.worker, tenantId: "other" }, { ...runtime.worker, workspaceId: "other" },
    { ...runtime.worker, runtimeId: "other" }, { ...runtime.worker, runtimeGeneration: 4 },
    { ...runtime.worker, expiresAt: now - 1 },
  ]) await assert.rejects(harness({ runtimeValue: { ...runtime, worker } }).connect(), rejected);
  await assert.rejects(harness({ runtimeValue: { ...runtime, deployment: "local" } }).connect(), rejected);
});

await check("replacement never reuses old bearer on an old or new target", async () => {
  let currentStatus = status, currentGrant = grant;
  const h = harness({ hostFetch: async url => Response.json(String(url).endsWith("/connection") ? currentGrant : currentStatus) });
  const client = await h.connect();
  const newHandle = { ...handle, generation: 4, machine_id: "vm_4" };
  const newLaunch = { ...launch, runtime_id: "worker_4" };
  currentStatus = { ...status, binding: { ...status.binding, allocation: { ...allocation, generation: 4, launch: newLaunch }, status: { ...status.binding.status, handle: newHandle, connection: { ...connection, handle: newHandle } } } };
  currentGrant = { ...grant, handle: newHandle, runtimeId: newLaunch.runtime_id, workerGeneration: 4 };
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    await assert.rejects(client.capabilities.list(), e => e.body?.code === "auth.token_resolution_failed");
    assert.equal(h.workerCalls.length, 1);
    const hostCalls = h.hostCalls.length;
    await assert.rejects(client.capabilities.list());
    assert.equal(h.hostCalls.length, hostCalls, "replacement poisons this connection until explicit reconnect");
  } finally { Date.now = originalNow; }
});

await check("new endpoint even at trusted origin requires explicit reconnect", async () => {
  let replacement = false;
  const h = harness({ hostFetch: async url => {
    if (String(url).endsWith("/connection")) return Response.json(replacement ? { ...grant, baseUrl: "https://worker.test/new/" } : grant);
    return Response.json(replacement ? { ...status, binding: { ...status.binding, status: { ...status.binding.status, connection: { ...connection, agent_api_base_url: "https://worker.test/new/" } } } } : status);
  } });
  const client = await h.connect(); replacement = true;
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    await assert.rejects(client.capabilities.list()); assert.equal(h.workerCalls.length, 1);
  } finally { Date.now = originalNow; }
});

await check("host redirects, wrong-origin responses and oversized bodies rejected", async () => {
  for (const response of [new Response("APP_AUTH_ONLY", { status: 302 }), new Response("x".repeat(65537)), new Response("{}", { headers: { "content-length": "65537" } })]) {
    const host = new HostWorkspaceClient({ ...baseOptions, fetch: async () => response });
    await assert.rejects(host.connection(workspace), rejected);
  }
  for (const flag of ["redirected", "url"]) {
    const response = Response.json(grant);
    Object.defineProperty(response, flag, { value: flag === "url" ? "https://foreign.test/" : true });
    const host = new HostWorkspaceClient({ ...baseOptions, fetch: async () => response });
    await assert.rejects(host.connection(workspace), rejected);
  }
});

await check("HTTP and SSE redirects are terminal and credential targets confined", async () => {
  for (const response of [new Response(null, { status: 307 }), (() => { const r = Response.json([]); Object.defineProperty(r, "url", { value: "https://foreign.test/" }); return r; })()]) {
    let calls = 0;
    const transport = createHttpTransport({ baseUrl: "https://worker.test/api/", token: "WORKER_ONLY", fetch: async (_, init) => {
      calls++; assert.equal(init.redirect, "manual"); assert.equal(init.credentials, "omit"); return response;
    } });
    await assert.rejects(transport.request({ path: "/v2/runtime" }), e => e.body?.code === "transport.unsafe_target");
    await assert.rejects(within(transport.events()[Symbol.asyncIterator]().next()), e => e.body?.code === "transport.unsafe_target");
    assert.equal(calls, 2, "no redirect/reconnect retries");
    for (const path of ["https://foreign.test/x", "//foreign.test/x", "../../x", "\\\\foreign.test/x", "/x?token=secret"]) await assert.rejects(transport.request({ path }));
    assert.equal(calls, 2);
  }
});

await check("host callback, fetch and stream hangs bounded; abort redacted", async () => {
  for (const patch of [
    { token: async () => new Promise(() => {}) },
    { fetch: async () => new Promise(() => {}) },
    { fetch: async () => new Response(new ReadableStream({ pull() { return new Promise(() => {}); } })) },
  ]) {
    const host = new HostWorkspaceClient({ ...baseOptions, timeoutMs: 5, ...patch });
    await assert.rejects(within(host.connection(workspace)), e => rejected(e) && e.code === "timeout");
  }
  const abort = new AbortController();
  const host = new HostWorkspaceClient({ ...baseOptions, token: async () => new Promise(() => {}) });
  const pending = host.connection(workspace, abort.signal);
  abort.abort("APP_AUTH_ONLY PRIVATE_SIGNER");
  await assert.rejects(within(pending), e => e.name === "AbortError" && !inspect(e).includes("APP_AUTH_ONLY"));
});

await check("host mutations never retry and worker WebSockets fail closed", async () => {
  let attempts = 0;
  const host = new HostWorkspaceClient({ ...baseOptions, fetch: async () => { attempts++; return new Response(null, { status: 503 }); } });
  await assert.rejects(host.destroy(workspace, handle), rejected); assert.equal(attempts, 1);
  const h = harness(); const client = await h.connect();
  // Socket APIs remain on the normal client; this helper deliberately offers HTTP/SSE only.
  assert.equal(h.workerCalls.length, 1);
  assert(!("cloud" in client));
});

console.log(`Host workspace broker and credential safety tests passed (${passed} scenarios)`);

// SSE reconnect resolves the broker cache at connection time, not stream creation.
{
  let streams = 0;
  const originalNow = Date.now;
  const h = harness({ workerFetch: async url => {
    if (String(url).endsWith("/v2/runtime")) return Response.json(runtime);
    streams++;
    return new Response(`data: ${JSON.stringify({ id: `event_${streams}`, sequence: streams, schemaVersion: "1", source: "test", timestamp: 1, type: "session.status", data: { sessionID: "one", status: { type: "idle" } } })}\n\n`);
  } });
  try {
    const client = await h.connect();
    const events = client.events.subscribe()[Symbol.asyncIterator]();
    assert.equal((await within(events.next())).value.sequence, 1);
    Date.now = () => (now + 280) * 1000;
    assert.equal((await within(events.next())).value.sequence, 2);
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 2);
    assert.equal(h.workerCalls.at(-1).headers["last-event-id"], "1");
    await events.return();
  } finally { Date.now = originalNow; }
}
// Worker handshake must be abort/deadline bounded even with malicious injected fetch.
{
  const h = harness({ workerFetch: async () => new Promise(() => {}) });
  await assert.rejects(within(connectWorkspaceWorker({ host: h.host, workspace, fetch: h.fetch, connectionTimeoutMs: 5 })), e => rejected(e) && e.code === "timeout");
  const abort = new AbortController();
  const pending = connectWorkspaceWorker({ host: h.host, workspace, fetch: h.fetch, signal: abort.signal });
  setTimeout(() => abort.abort("WORKER_ONLY APP_AUTH_ONLY"), 5);
  await assert.rejects(within(pending), e => e.name === "AbortError" && !inspect(e).includes("WORKER_ONLY"));
}
console.log("Host SSE broker refresh and bounded worker handshake tests passed");

// A changed origin is rejected even when both origins were explicitly trusted.
{
  let replacement = false, workerRequests = 0;
  const host = new HostWorkspaceClient({ ...baseOptions, trustedWorkerOrigins: ["https://worker.test", "https://other-worker.test"], fetch: async url => {
    const endpoint = replacement ? "https://other-worker.test/api/" : grant.baseUrl;
    return Response.json(String(url).endsWith("/connection") ? { ...grant, baseUrl: endpoint } : { ...status, binding: { ...status.binding, status: { ...status.binding.status, connection: { ...connection, agent_api_base_url: endpoint } } } });
  } });
  const client = await connectWorkspaceWorker({ host, workspace, fetch: async (url, init) => {
    workerRequests++; assert.equal(new URL(url).origin, "https://worker.test");
    assert.equal(init.headers.authorization, "Bearer WORKER_ONLY");
    return Response.json(runtime);
  } });
  replacement = true;
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    await assert.rejects(client.runtime.get()); assert.equal(workerRequests, 1);
  } finally { Date.now = originalNow; }
}
// Aborting one refresh waiter cannot cancel another waiter or trigger a second flight.
{
  let release, hold = false;
  const gate = new Promise(resolve => { release = resolve; });
  const h = harness({ hostFetch: async url => {
    if (hold && String(url).endsWith("/status")) await gate;
    return Response.json(String(url).endsWith("/connection") ? grant : status);
  } });
  const client = await h.connect(); hold = true;
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    const abort = new AbortController();
    const one = client.runtime.get({ signal: abort.signal });
    const two = client.capabilities.list();
    await new Promise(resolve => setTimeout(resolve, 0));
    abort.abort("WORKER_ONLY");
    await assert.rejects(within(one), e => e.name === "AbortError" && !inspect(e).includes("WORKER_ONLY"));
    release(); await within(two);
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/status")).length, 2);
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 2);
  } finally { release(); Date.now = originalNow; }
}
console.log("Host changed-origin fencing and singleflight abort isolation tests passed");

// Match service-api VM namespace rules, independently of the SDK host OS.
{
  const { normalizedVmNamespace, sameVmNamespace } = await import("../packages/http/dist/namespace.js");
  for (const path of ["/vm/workspace", "C:\\Workspace", "c:/workspace/child", String.raw`\\?\C:\Workspace`, "C:/éé", "/vm/colon:and?posix."]) assert.notEqual(normalizedVmNamespace(path), undefined, path);
  for (const path of [
    "/", "//vm/workspace", "/vm/", "/vm//child", "/vm/./child", "/vm/../child", "relative", "C:", "C:/", "C:workspace",
    String.raw`\workspace`, String.raw`\\host\share`, String.raw`\\.\C:\workspace`, String.raw`\\?\UNC\host\share`,
    "C:/work/../escape", "C:/work/./child", "C:/work//child", "C:/work/child:stream", "C:/work/NUL.txt", "C:/work/COM1.txt",
    "C:/work/trailing.", "C:/work/trailing ", "/vm/\0child", "/vm\\child", "/" + "é".repeat(2048),
  ]) assert.equal(normalizedVmNamespace(path), undefined, path);
  assert(sameVmNamespace("C:/Workspace", String.raw`\\?\c:\WORKSPACE`));
  assert(sameVmNamespace(String.raw`c:\Workspace\Child`, "C:/workspace/CHILD"));
  for (const [a, b] of [["C:/workspace", "D:/workspace"], ["C:/work", "C:/workspace"], ["/Workspace", "/workspace"], ["C:/É", "C:/é"], ["C:/work", "/work"], ["C:/work", "C:/work/../work"]]) assert(!sameVmNamespace(a, b));
}
// Grants and worker introspection may spell the same Windows root differently.
{
  const windowsLaunch = { ...launch, root: "C:/Workspace", state_root: "D:/State" };
  const windowsStatus = { ...status, binding: { ...status.binding, allocation: { ...allocation, launch: windowsLaunch } } };
  for (const grantRoot of ["c:/WORKSPACE", String.raw`C:\Workspace`, String.raw`\\?\c:\workspace`]) {
    const h = harness({ statusValue: windowsStatus, grantValue: { ...grant, root: grantRoot }, runtimeValue: { ...runtime, worker: { ...runtime.worker, root: String.raw`\\?\C:\WORKSPACE` } } });
    await h.connect();
  }
  for (const grantRoot of ["D:/Workspace", "C:/Different", "C:/Workspace/Child", "/Workspace", String.raw`\\?\C:\Workspace\..\Workspace`]) {
    const h = harness({ statusValue: windowsStatus, grantValue: { ...grant, root: grantRoot } });
    await assert.rejects(h.connect(), rejected); assert.equal(h.workerCalls.length, 0);
  }
  for (const workerRoot of ["D:/Workspace", "C:/Different", "C:/Workspace/Child", "C:/Workspace/../Workspace", null]) {
    const h = harness({ statusValue: windowsStatus, grantValue: { ...grant, root: "c:/WORKSPACE" }, runtimeValue: { ...runtime, worker: { ...runtime.worker, root: workerRoot } } });
    await assert.rejects(h.connect(), rejected);
  }
  // Equivalent spellings on refresh don't look like a replacement; real root changes do.
  let spelling = "C:/Workspace";
  const h = harness({ hostFetch: async url => Response.json(String(url).endsWith("/connection") ? { ...grant, root: spelling } : windowsStatus), runtimeValue: { ...runtime, worker: { ...runtime.worker, root: "c:/workspace" } } });
  const client = await h.connect();
  const originalNow = Date.now;
  try {
    Date.now = () => (now + 280) * 1000;
    spelling = String.raw`\\?\c:\WORKSPACE`;
    await client.capabilities.list();
    assert.equal(h.hostCalls.filter(c => c.url.endsWith("/connection")).length, 2);
  } finally { Date.now = originalNow; }
}
console.log("POSIX/Windows/verbatim namespace validation and broker root-equivalence tests passed");
