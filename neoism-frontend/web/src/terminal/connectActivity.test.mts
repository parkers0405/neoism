import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

test("catalog terminal failure retains its inner correlation through the browser host", () => {
  const panel: any = Object.create(TerminalPanel.prototype);
  const forwarded: unknown[] = [];
  panel.syncAgentCatalogEndpoint = () => {};
  panel.scheduleDraw = () => {};
  panel.wasmAdapter = { agentEvent: (wire: string) => forwarded.push(JSON.parse(wire)) };
  const payload = { ConnectProviderCatalogFailed: { request_id: "catalog-17", error: "HTTP 503" } };
  panel.agentReply(payload);
  assert.deepEqual(forwarded, [payload]);
});

test("provider browser launch is retained and blocked popups still forward inline fallback data", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  try {
    for (const blocked of [false, true]) {
      const panel: any = Object.create(TerminalPanel.prototype);
      const launched: string[] = [];
      const forwarded: unknown[] = [];
      Object.defineProperty(globalThis, "window", { configurable: true, value: {
        open: (url: string) => { launched.push(url); if (blocked) throw new Error("popup blocked"); return null; },
      } });
      panel.syncAgentCatalogEndpoint = () => {};
      panel.scheduleDraw = () => {};
      panel.wasmAdapter = { agentEvent: (wire: string) => forwarded.push(JSON.parse(wire)) };
      const payload = { ConnectOauthUrl: { url: "https://example.invalid/sign-in?state=opaque", auto: true, instructions: "Enter code ABCD", attempt_id: "attempt-1" } };
      panel.agentReply(payload);
      assert.deepEqual(launched, [payload.ConnectOauthUrl.url]);
      assert.deepEqual(forwarded, [payload], "a popup failure must not swallow the inline URL/instructions");
    }
  } finally {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else delete (globalThis as any).window;
  }
});

test("transport interruption releases provider reply guards only after sends are disabled", () => {
  const panel: any = Object.create(TerminalPanel.prototype);
  const calls: string[] = [];
  panel.blameConnectionEpoch = 0;
  panel.transportAvailable = true;
  panel.notesWorkspaceBarrierReady = true;
  panel.crdtTransportAvailable = true;
  panel.pendingReconnectSnapshots = new Map([["old", {}]]);
  panel.pendingClipboardImages = new Map([["old", {}]]);
  panel.pendingTerminalTabSpawns = ["old"];
  panel.notesRefreshTimer = null;
  panel.pendingServiceMappers = new Map([[7, {}]]);
  panel.wasmAdapter = {
    agentConnectTransportClosed: () => {
      assert.equal(panel.transportAvailable, false);
      calls.push("provider-transport-closed");
    },
    serviceReply: (id: number, reply: any) => {
      assert.equal(id, 7);
      assert.equal(reply.Error.message, "Connection lost");
      calls.push("service-error");
    },
  };
  panel.scheduleDraw = () => calls.push("redraw");

  panel.connectionInterrupted("Connection lost");

  assert.deepEqual(calls, ["provider-transport-closed", "service-error", "redraw"]);
  assert.equal(panel.pendingServiceMappers.size, 0);
  assert.equal(panel.pendingReconnectSnapshots.size, 0);
  assert.equal(panel.crdtTransportAvailable, false);
});
