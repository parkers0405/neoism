#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const generator = fileURLToPath(new URL("./generate-contract.mjs", import.meta.url));
const document = {
  openapi: "3.1.1",
  info: { title: "test", version: "1" },
  paths: {
    "/v2/items/{id}": {
      parameters: [{ name: "id", in: "path", required: true, schema: { type: "string" } }],
      post: {
        operationId: "v2.items.create",
        parameters: [
          { name: "limit", in: "query", schema: { type: "integer" } },
          { name: "X-Test", in: "header", required: true, schema: { type: "string" } },
        ],
        requestBody: { required: true, content: { "application/json": { schema: { $ref: "#/components/schemas/PromptRequest" } } } },
        responses: {
          200: { description: "json", content: { "application/json": { schema: { type: ["string", "null"] } } } },
          206: { description: "bytes", content: { "application/octet-stream": { schema: { type: "string", format: "binary" } } } },
        },
      },
    },
    "/v2/events": { get: { operationId: "v2.events", responses: { 200: { description: "sse", content: { "text/event-stream": { schema: { type: "string" } } } } } } },
    "/v2/socket": { get: { operationId: "v2.socket", "x-neoism-transport": "websocket", responses: { 101: { description: "upgrade" } } } },
  },
  components: { schemas: {
    PromptRequest: {
      type: "object",
      properties: { prompt: { type: "string" }, parts: { type: "array", items: {} } },
      anyOf: [
        { type: "object", required: ["prompt"], properties: { prompt: { type: "string" } } },
        { type: "object", required: ["parts"], properties: { parts: { type: "array", items: {} } } },
      ],
    },
    SiblingUnion: { type: "object", properties: { id: { type: "string" } }, anyOf: [{ required: ["id"], type: "object", properties: { id: { type: "string" } } }] },
  } },
};

const result = spawnSync(process.execPath, [generator], {
  input: JSON.stringify(document), encoding: "utf8",
});
assert.equal(result.status, 0, result.stderr);
const output = result.stdout;
assert.match(output, /PromptRequest = .*& \(\(\{ prompt: string;/);
assert.match(output, /path: \{ id: string; \}/);
assert.match(output, /query\?: \{ limit\?: number; \}/);
assert.match(output, /headers: \{ "X-Test": string; \}/);
assert.match(output, /responses: \{ "200": string \| null; "206": Uint8Array; \}/);
assert.match(output, /"v2.events": \{"method":"GET","path":"\/v2\/events","transport":"sse"/);
assert.match(output, /"v2.socket": \{"method":"GET","path":"\/v2\/socket","transport":"websocket"/);
console.log("contract generator tests passed");