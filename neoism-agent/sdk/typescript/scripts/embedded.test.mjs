import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createEmbeddedNeoism } from "../packages/node/dist/index.js";

test("real embedded SDK: same engine, streaming, binary bodies, abort, auth, close and reopen", { timeout: 60_000 }, async () => {
  const root = await mkdtemp(join(tmpdir(), "neoism-sdk-embedded-"));
  const originalEnv = { ...process.env };
  process.env.XDG_CONFIG_HOME = join(root, "config");
  process.env.XDG_STATE_HOME = join(root, "state");
  process.env.NEOISM_AGENT_STATE_DIR = join(root, "state");
  process.env.NEOISM_AGENT_AUTH_PATH = join(root, "auth.json");
  process.env.NEOISM_AGENT_TOKEN = "embedded-test-credential";
  delete process.env.NEOISM_AGENT_AUTH_CONFIG;
  await mkdir(join(root, "workspace"));
  await writeFile(join(root, "workspace/input.txt"), "existing runtime tool result");
  const options = { database: join(root, "agent.db"), token: "embedded-test-credential", config: { model: "neoism/stub" } };
  let host;
  try {
    host = await createEmbeddedNeoism(options);
    const { client } = host;
    assert.equal((await client.meta.get()).apiVersion, "2.0.0");
    assert.equal((await client.config.defaults(join(root, "workspace"))).model, "neoism/stub");
    await assert.rejects(host.fetch("https://example.com/v2/meta"), /another origin/);
    const unauthorized = await host.fetch("http://neoism.embedded.invalid/v2/sessions");
    assert.equal(unauthorized.status, 401);
    await unauthorized.body.cancel();
    const auth = { authorization: "Bearer embedded-test-credential" };
    const streamAbort = new AbortController();
    const streamResponse = await host.fetch("http://neoism.embedded.invalid/v2/events?tail=true", { headers: auth, signal: streamAbort.signal });
    const stream = streamResponse.body.getReader();
    const session = await client.sessions.create({ directory: join(root, "workspace"), title: "Embedded SDK" });
    let eventText = "";
    while (!eventText.includes("session.created")) {
      const chunk = await stream.read();
      assert.equal(chunk.done, false);
      eventText += new TextDecoder().decode(chunk.value);
    }
    streamAbort.abort();
    await assert.rejects(stream.read(), error => error.name === "AbortError");
    for (const text of ["read-tool: input.txt", "second native SDK turn"]) {
      await client.sessions.prompt(session.id, { model: { providerId: "neoism", modelId: "stub" }, parts: [{ type: "text", text }] });
      await client.transport.request({ method: "POST", path: `/v2/sessions/${session.id}/wait` });
    }
    const messages = await client.sessions.messages(session.id);
    assert.ok(messages.items.length >= 5);
    assert.match(JSON.stringify(messages), /Tool result received/);
    assert.match(JSON.stringify(messages), /existing runtime tool result/);
    const data = Uint8Array.from({ length: 180_000 }, (_, index) => index % 251);
    const artifact = await client.artifacts.upload(data, { filename: "binary.dat", mediaType: "application/octet-stream", sessionId: session.id });
    assert.deepEqual(await client.artifacts.download(artifact.id), data);
    await client.artifacts.delete(artifact.id);
    // More than the native admission limit: EOF/204/cancel must release slots.
    for (let index = 0; index < 70; index++) {
      await client.health.get();
      const controller = new AbortController();
      const response = await host.fetch("http://neoism.embedded.invalid/v2/events?tail=true", { headers: auth, signal: controller.signal });
      controller.abort();
      await assert.rejects(response.body.getReader().read(), error => error.name === "AbortError");
    }
    const events = client.events.subscribe({ tail: true })[Symbol.asyncIterator]();
    const nextEvent = events.next().catch(error => ({ error }));
    await host.close();
    await host.close();
    await nextEvent;
    await assert.rejects(client.meta.get(), error => error.name === "AbortError");
    host = await createEmbeddedNeoism(options);
    assert.deepEqual(await host.client.sessions.messages(session.id), messages);
    await host[Symbol.asyncDispose]();
  } finally {
    if (host) await host.close();
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv);
    await rm(root, { recursive: true, force: true });
  }
});