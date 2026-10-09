import assert from "node:assert/strict";
import { inspect } from "node:util";
import { CloudRuntimeClient, createHttpTransport, NeoismApiError } from "../packages/http/dist/index.js";

let passed = 0;
async function check(name, run) {
  await run();
  passed++;
}
async function within(pending) {
  let timer;
  try {
    return await Promise.race([pending, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("Token test exceeded deadline")), 1000);
    })]);
  } finally { clearTimeout(timer); }
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
const jsonResponse = () => Response.json({ ok: true });

await check("resolves fresh credentials for every HTTP request", async () => {
  const signal = new AbortController().signal;
  const calls = [];
  let resolutions = 0;
  const transport = createHttpTransport({
    baseUrl: "https://worker.test/api/",
    token: async received => {
      assert.equal(received, signal);
      return `WORKER_SHORT_LIVED_${++resolutions}`;
    },
    fetch: async (url, init) => { calls.push({ url: String(url), ...init }); return jsonResponse(); },
  });
  assert.equal(resolutions, 0, "construction must not request credentials");
  await transport.request({ path: "/v2/runtime", signal });
  await transport.request({ path: "/v2/sessions", method: "POST", body: {}, signal });
  assert.equal(resolutions, 2);
  assert.deepEqual(calls.map(call => call.headers.authorization), ["Bearer WORKER_SHORT_LIVED_1", "Bearer WORKER_SHORT_LIVED_2"]);
  assert.deepEqual(calls.map(call => call.url), ["https://worker.test/api/v2/runtime", "https://worker.test/api/v2/sessions"]);
});

await check("static credentials remain supported", async () => {
  const headers = [];
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: "STATIC_WORKER", fetch: async (_, init) => {
    headers.push(init.headers); return jsonResponse();
  } });
  await transport.request({ path: "/one" });
  await transport.request({ path: "/two" });
  assert.deepEqual(headers.map(h => h.authorization), ["Bearer STATIC_WORKER", "Bearer STATIC_WORKER"]);
});

await check("absent and undefined-resolved credentials send no authorization", async () => {
  for (const token of [undefined, () => undefined, async () => undefined]) {
    const transport = createHttpTransport({ baseUrl: "https://worker.test", ...(token === undefined ? {} : { token }), fetch: async (_, init) => {
      assert.equal(new Headers(init.headers).has("authorization"), false); return jsonResponse();
    } });
    await transport.request({ path: "/public" });
  }
});

await check("explicit common and per-request authorization overrides are case insensitive", async () => {
  let resolutions = 0;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { resolutions++; return "TOKEN_SOURCE"; },
    headers: { Authorization: "Bearer EXPLICIT_COMMON" },
    fetch: async (_, init) => {
      assert.equal(new Headers(init.headers).get("authorization"), resolutions === 1 ? "Bearer EXPLICIT_COMMON" : "Bearer EXPLICIT_REQUEST");
      assert.equal(Object.keys(init.headers).filter(k => k.toLowerCase() === "authorization").length, 1);
      return jsonResponse();
    },
  });
  await transport.request({ path: "/one" });
  await transport.request({ path: "/two", headers: { AUTHORIZATION: "Bearer EXPLICIT_REQUEST" } });
  assert.equal(resolutions, 2, "resolve once per request even if explicit authorization overrides it");
});

await check("pre-aborted resolution never invokes callback or fetch", async () => {
  const abort = new AbortController();
  abort.abort("BROKER_SECRET");
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { assert.fail("resolver started"); }, fetch: async () => { assert.fail("fetch started"); } });
  await assert.rejects(transport.request({ path: "/one", signal: abort.signal }), error => error.name === "AbortError" && !inspect(error).includes("BROKER_SECRET"));
});

await check("aborting an unresolved callback promptly rejects without fetch", async () => {
  const abort = new AbortController();
  const started = deferred();
  const token = deferred();
  let fetched = false;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: signal => {
    assert.equal(signal, abort.signal); started.resolve(); return token.promise;
  }, fetch: async () => { fetched = true; return jsonResponse(); } });
  const pending = transport.request({ path: "/one", signal: abort.signal });
  await started.promise;
  abort.abort("BROKER_SECRET");
  await assert.rejects(within(pending), error => error.name === "AbortError" && !inspect(error).includes("BROKER_SECRET"));
  token.resolve("LATE_WORKER_SECRET");
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(fetched, false);
});

await check("late resolver rejection after abort is observed and never forwarded", async () => {
  const abort = new AbortController();
  const started = deferred();
  const token = deferred();
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { started.resolve(); return token.promise; }, fetch: async () => { assert.fail("fetch started"); } });
  const pending = transport.request({ path: "/one", signal: abort.signal });
  await started.promise;
  abort.abort();
  await assert.rejects(within(pending), { name: "AbortError" });
  token.reject(new Error("BROKER_SECRET"));
  await new Promise(resolve => setTimeout(resolve, 0));
});

await check("resolver failures are sanitized and do not trigger fetch or retries", async () => {
  for (const token of [() => { throw new Error("BROKER_SECRET https://private.test"); }, async () => { throw new Error("BROKER_SECRET https://private.test"); }]) {
    const transport = createHttpTransport({ baseUrl: "https://worker.test", token, fetch: async () => { assert.fail("fetch started"); } });
    await assert.rejects(transport.request({ path: "/mutate", method: "POST", body: {} }), error => {
      assert(error instanceof NeoismApiError);
      assert.equal(error.status, 401);
      assert.equal(error.body.code, "auth.token_resolution_failed");
      assert.equal(error.body.retryable, false);
      assert.equal(error.cause, undefined);
      assert(!inspect(error).includes("BROKER_SECRET"));
      assert(!inspect(error).includes("private.test"));
      return true;
    });
  }
});

await check("HTTP 401 never retries a mutation or refreshes it automatically", async () => {
  let resolutions = 0, fetches = 0;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => `TOKEN_${++resolutions}`, fetch: async () => {
    fetches++;
    return Response.json({ code: "auth.expired", message: "Expired", retryable: false, details: {} }, { status: 401 });
  } });
  await assert.rejects(transport.request({ path: "/mutate", method: "POST", body: {} }), error => error instanceof NeoismApiError && error.status === 401);
  assert.equal(resolutions, 1);
  assert.equal(fetches, 1);
});

const event = (id, sequence) => ({ id, sequence, schemaVersion: "1", source: "test", timestamp: 1, type: "session.status", data: { sessionID: "session-1", status: { type: "idle" } } });
function streamResponse(events) {
  return new Response(events.map(e => `data: ${JSON.stringify(e)}\n\n`).join(""), { headers: { "content-type": "text/event-stream" } });
}
await check("SSE static strings remain supported", async () => {
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: "STATIC_WORKER", fetch: async (_, init) => {
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer STATIC_WORKER");
    return streamResponse([event("evt_static", 1)]);
  } });
  const events = transport.events()[Symbol.asyncIterator]();
  assert.deepEqual((await within(events.next())).value, event("evt_static", 1));
  await events.return();
});

await check("SSE absent credentials send no authorization", async () => {
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: async () => undefined, fetch: async (_, init) => {
    assert.equal(new Headers(init.headers).has("authorization"), false);
    return streamResponse([event("evt_public", 1)]);
  } });
  const events = transport.events()[Symbol.asyncIterator]();
  assert.deepEqual((await within(events.next())).value, event("evt_public", 1));
  await events.return();
});

await check("SSE expiry closes and reconnects with fresh credentials while preserving resume/dedupe", async () => {
  const abort = new AbortController();
  let resolutions = 0;
  const calls = [];
  const first = event("evt_10", 10), older = event("evt_8", 8), next = event("evt_11", 11);
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: async signal => {
    assert.equal(signal, abort.signal); return `WORKER_FRESH_${++resolutions}`;
  }, fetch: async (_, init) => {
    calls.push(init);
    if (calls.length === 1) return streamResponse([first]); // Server closes stream at token expiry.
    if (calls.length === 2) return streamResponse([first, older, next]);
    assert.fail("unexpected reconnect");
  } });
  const events = transport.events({ signal: abort.signal, tail: true })[Symbol.asyncIterator]();
  assert.equal(resolutions, 0, "credentials are resolved when connecting, not when constructing the stream");
  assert.deepEqual((await within(events.next())).value, first);
  assert.deepEqual((await within(events.next())).value, older, "out-of-order fresh events must not be dropped");
  assert.deepEqual((await within(events.next())).value, next);
  assert.equal(resolutions, 2);
  assert.deepEqual(calls.map(call => call.headers.authorization), ["Bearer WORKER_FRESH_1", "Bearer WORKER_FRESH_2"]);
  assert.equal(calls[0].headers["last-event-id"], undefined);
  assert.equal(calls[1].headers["last-event-id"], "10");
  abort.abort();
  await events.return();
});

await check("SSE explicit authorization overrides fresh tokens on each reconnect", async () => {
  const abort = new AbortController();
  let resolutions = 0, fetches = 0;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => `TOKEN_${++resolutions}`, headers: { AUTHORIZATION: "Bearer COMMON" }, fetch: async (_, init) => {
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer COMMON");
    return streamResponse([event(`evt_${++fetches}`, fetches)]);
  } });
  const events = transport.events({ signal: abort.signal })[Symbol.asyncIterator]();
  await within(events.next()); await within(events.next());
  assert.equal(resolutions, 2);
  abort.abort(); await events.return();
});

await check("aborted SSE token resolution never starts fetch", async () => {
  const abort = new AbortController();
  const started = deferred();
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: signal => {
    assert.equal(signal, abort.signal); started.resolve(); return new Promise(() => {});
  }, fetch: async () => { assert.fail("fetch started"); } });
  const events = transport.events({ signal: abort.signal })[Symbol.asyncIterator]();
  const pending = events.next();
  await started.promise; abort.abort();
  assert.deepEqual(await within(pending), { value: undefined, done: true });
});

await check("SSE resolver failure is sanitized and terminal", async () => {
  let resolutions = 0;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { resolutions++; throw Error("BROKER_SECRET"); }, fetch: async () => { assert.fail("fetch started"); } });
  await assert.rejects(within(transport.events()[Symbol.asyncIterator]().next()), error => error instanceof NeoismApiError && !inspect(error).includes("BROKER_SECRET"));
  assert.equal(resolutions, 1);
});

await check("independent lifecycle transport never invokes worker token callback", async () => {
  let resolutions = 0;
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { resolutions++; return "WORKER_ONLY"; }, fetch: async () => jsonResponse() });
  const allocation = { owner: { tenant: "tenant:example", workspace: "workspace:one" }, generation: 1, provider: "bridge", spec: { image: "worker", region: "us", vcpus: 2, memory_mib: 2048, disk_gib: 20 } };
  const handle = { owner: allocation.owner, generation: 1, provider: "bridge", machine_id: "machine" };
  const cloud = new CloudRuntimeClient({ endpoint: "https://bridge.test", provider: "bridge", bearer: "HOST_ONLY", capabilities: { isolation: "virtual_machine", stop_start: true, cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true }, fetch: async (_, init) => {
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer HOST_ONLY");
    assert(!JSON.stringify(init).includes("WORKER_ONLY"));
    return Response.json({ version: 2, status: { handle, state: "stopped", ready: false } });
  } });
  await cloud.ensure(allocation);
  assert.equal(resolutions, 0);
  await transport.request({ path: "/v2/runtime" });
  assert.equal(resolutions, 1);
});

await check("WebSocket setup never resolves or appends bearer tokens", async () => {
  let resolutions = 0;
  class FakeWebSocket extends EventTarget {
    constructor() { super(); queueMicrotask(() => this.dispatchEvent(new Event("open"))); }
    close() { this.dispatchEvent(new Event("close")); }
    send() {}
  }
  const transport = createHttpTransport({ baseUrl: "https://worker.test", token: () => { resolutions++; return "WORKER_ONLY"; }, webSocket: url => {
    assert.equal(url, "wss://worker.test/socket"); return new FakeWebSocket();
  } });
  const socket = await transport.connectSocket({ path: "/socket" });
  assert.equal(resolutions, 0); socket.close();
});

console.log(`HTTP token refresh tests passed (${passed} scenarios)`);
