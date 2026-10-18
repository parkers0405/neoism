import assert from "node:assert/strict";
import test from "node:test";
import { detectClientOs } from "./clientOs.ts";
import { remoteCursorProjection } from "./projections.ts";
import { PresencePublisher } from "./PresencePublisher.ts";
import type { WasmInputPolicyModule } from "../terminal/createTerminal.ts";

test("browser OS uses client hints then platform then UA, unknown stays null", () => {
  assert.equal(detectClientOs({ userAgentData: { platform: "Windows" }, platform: "MacIntel" }), "windows");
  assert.equal(detectClientOs({ platform: "Win32" }), "windows");
  assert.equal(detectClientOs({ userAgentData: { platform: "" }, userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)" }), "windows");
  assert.equal(detectClientOs({ platform: "MacIntel", userAgent: "Windows" }), "macos");
  assert.equal(detectClientOs({ userAgent: "Android 14; Linux" }), "android");
  assert.equal(detectClientOs({ userAgent: "iPhone; CPU iPhone OS 18 like Mac OS X" }), "ios");
  assert.equal(detectClientOs({ platform: "Linux x86_64" }), "linux");
  assert.equal(detectClientOs({}), null);
  assert.equal(detectClientOs({ platform: "unrecognized" }), null);
});

test("code and markdown projection retains optional OS and defaults older peers", () => {
  const peer = { buffer_id: "file:///a", peer_id: "peer", display_name: "Peer", color: { r: 1, g: 2, b: 3 }, cursor: { line: 4, column: 5 }, updated_at_ms: 0 };
  assert.equal(remoteCursorProjection(peer).client_os, null);
  assert.deepEqual(remoteCursorProjection({ ...peer, client_os: "windows", insert: false }), {
    name: "Peer", color: [1, 2, 3], rainbow: false, client_os: "windows", insert: false, line: 4, col_utf16: 5,
  });
});

test("OS setter buffers before WASM, applies latest value, and clears with null", () => {
  let loaded: WasmInputPolicyModule | null = null;
  const calls: Array<string | null> = [];
  const pub = new PresencePublisher("me", "Browser", 75, 4000, () => loaded);
  pub.setClientOs("linux");
  pub.setClientOs("windows");
  assert.deepEqual(pub.tick(null, 1), []);
  assert.equal(calls.length, 0);
  loaded = { PresencePublisherBridge: class {
    set_client_os(os: string | null) { calls.push(os); }
    tick() { return []; }
  } } as unknown as WasmInputPolicyModule;
  pub.tick(null, 2);
  pub.tick(null, 3);
  assert.deepEqual(calls, ["windows"]);
  pub.setClientOs(null);
  assert.deepEqual(calls, ["windows", null]);
});
