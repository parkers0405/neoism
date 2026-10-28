import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AgentSdk, agentSdkEndpoint, sdkModel, sdkPrompt } from "../src/daemon/sdk";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((close) => close()));

test("SDK endpoints preserve the daemon origin and proxy prefix with an explicit workspace", () => {
  expect(agentSdkEndpoint("wss://host.example/neoism/session?token=secret", "space/a")).toEqual({
    baseUrl: "https://host.example/neoism/agent/workspaces/space%2Fa/", socket: undefined, token: "secret",
  });
  expect(agentSdkEndpoint("ws+unix:///run/user/1000/neoism.sock:/session", "a")).toEqual({
    baseUrl: "http://localhost/agent/workspaces/a/", socket: "/run/user/1000/neoism.sock", token: undefined,
  });
  expect(() => agentSdkEndpoint("ws+unix:///missing-separator", "a")).toThrow();
});

test("the real SDK sends prompts through the scoped Unix proxy with header authentication and exact attachment bytes", async () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-sdk-"));
  cleanups.push(() => rmSync(directory, { recursive: true, force: true }));
  const socket = join(directory, "daemon.sock");
  let received: { path: string; authorization: string | null; body: any } | undefined;
  const server = Bun.serve({ unix: socket, fetch: async (request) => {
    received = { path: new URL(request.url).pathname, authorization: request.headers.get("authorization"), body: await request.json() };
    return new Response(null, { status: 204 });
  } });
  cleanups.push(() => server.stop(true));
  const sdk = new AgentSdk({ endpoint: `ws+unix://${socket}:/session`, authToken: "operator", pairingToken: "device" });
  cleanups.push(() => sdk.dispose());
  const selection = { agent: "plan", model: "openai/model/variant", thinking: "high" };
  await sdk.forWorkspace("a").sessions.prompt("session/one", sdkPrompt("Review", [{ kind: "image/png", path: "capture.png", bytes: [0, 127, 128, 255] }], selection, "queue"));
  expect(received?.path).toBe("/agent/workspaces/a/v2/sessions/session%2Fone/prompt");
  expect(received?.authorization).toBe("Bearer device");
  expect(received?.body).toMatchObject({ agent: "plan", delivery: "queue", model: { providerId: "openai", modelId: "model/variant", variant: "high" },
    parts: [{ type: "text", text: "Review" }, { type: "file", filename: "capture.png", mime: "image/png", url: "data:image/png;base64,AH+A/w==" }] });
  expect(sdk.forWorkspace("a")).toBe(sdk.forWorkspace("a"));
  expect(sdk.forWorkspace("b")).not.toBe(sdk.forWorkspace("a"));
  expect(sdkModel(selection)).toEqual({ providerId: "openai", id: "model/variant", variant: "high" });
});

test("SDK mutations are never replayed on transport failure and disposal aborts pending requests", async () => {
  let attempts = 0, aborted = false;
  const fetcher = (async (_input: unknown, init?: RequestInit) => {
    attempts++;
    return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener("abort", () => { aborted = true; reject(new Error("aborted")); }, { once: true }));
  }) as typeof fetch;
  const sdk = new AgentSdk({ endpoint: "ws://127.0.0.1:7878/session" }, fetcher);
  const request = sdk.forWorkspace("a").sessions.abort("one");
  await Bun.sleep(5); sdk.dispose();
  await expect(request).rejects.toThrow();
  expect(aborted).toBe(true); expect(attempts).toBe(1);
});
