import { test } from "node:test";
import assert from "node:assert/strict";
import { ProtocolClient } from "./ProtocolClient.ts";
import type { AgentServerMessage } from "./types.ts";

test("project discovery, pages and failures reach WASM unchanged", () => {
  const received: Array<[number, AgentServerMessage]> = [];
  const client = new ProtocolClient({ url: "ws://authorized/session" }, {
    onAgentReply: (id, payload) => received.push([id, payload]),
  });
  const messages: AgentServerMessage[] = [
    { SessionProjects: { catalog_identity: "endpoint/root/epoch", generation: 88,
      items: [{ directory: "/one project", updatedMs: 123 }] } },
    { ProjectSessionPage: { catalog_identity: "endpoint/root/epoch", directory: "/one project",
      generation: 2, requested_cursor: "opaque+/=", threads: [], next_cursor: "next:3" } },
    { ProjectSessionPageFailed: { catalog_identity: "endpoint/root/epoch", directory: "/other",
      generation: 1, requested_cursor: null, message: "denied" } },
    { SessionProjectsFailed: { catalog_identity: "old endpoint", generation: 87, message: "offline" } },
    "SessionProjectsChanged",
  ];
  const receiver = client as unknown as { handleRawMessage(raw: string): void; authenticated: boolean };
  receiver.authenticated = true;
  messages.forEach((message, index) => receiver.handleRawMessage(JSON.stringify({
    AgentReply: { request_id: index, message },
  })));
  assert.deepEqual(received, messages.map((message, index) => [index, message]));
});
