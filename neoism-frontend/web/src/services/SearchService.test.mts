import assert from "node:assert/strict";
import test from "node:test";

import { SearchService, type SearchBridge } from "./SearchService.ts";
import type { ProtocolClient } from "../workspace/ProtocolClient.ts";

test("disconnect settles wasm search loading slots", () => {
  let collect: ((requestId: number, json: string) => void) | undefined;
  const replies: Array<[number, unknown]> = [];
  const bridge: SearchBridge = {
    setSearchCollectFiles: (callback) => { collect = callback; },
    serviceReply: (requestId, payload) => replies.push([requestId, payload]),
  };
  const client = {
    sendSearch: () => true,
  } as unknown as ProtocolClient;
  const service = new SearchService(client, bridge);
  service.install();
  collect?.(42, JSON.stringify({ CollectFiles: { cwd: "/workspace" } }));

  service.rejectPending(new Error("connection interrupted"));
  assert.deepEqual(replies, [[42, {
    SearchError: { req_id: 42, message: "connection interrupted" },
  }]]);
  service.rejectPending(new Error("again"));
  assert.equal(replies.length, 1, "settled bridge slots are removed");
});