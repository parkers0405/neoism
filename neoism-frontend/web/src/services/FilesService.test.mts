import assert from "node:assert/strict";
import test from "node:test";

import { DaemonFilesService } from "./FilesService";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { FilesClientMessage, FilesServerMessage } from "../workspace/types";

test("picker discovers daemon-advertised canonical locations", async () => {
  const sent: FilesClientMessage[] = [];
  const client = {
    async requestFiles(message: FilesClientMessage): Promise<FilesServerMessage> {
      sent.push(message);
      return {
        BrowserLocations: {
          locations: [{ kind: "home", label: "Home", path: "/home/test" }],
        },
      };
    },
  } as unknown as ProtocolClient;
  const locations = await new DaemonFilesService(client).browserLocations();
  assert.deepEqual(sent, ["ListBrowserLocations"]);
  assert.equal(locations[0]?.path, "/home/test");
});

test("picker sends canonical path and never a display label", async () => {
  const sent: FilesClientMessage[] = [];
  const client = {
    async requestFiles(message: FilesClientMessage): Promise<FilesServerMessage> {
      sent.push(message);
      return { DirListing: { path: "/home/test/Documents", entries: [] } };
    },
  } as unknown as ProtocolClient;
  await new DaemonFilesService(client).browserListDir("/home/test/Documents");
  assert.deepEqual(sent, [{ BrowserListDir: { path: "/home/test/Documents" } }]);
  assert.notEqual((sent[0] as { BrowserListDir: { path: string } }).BrowserListDir.path, "Documents");
});