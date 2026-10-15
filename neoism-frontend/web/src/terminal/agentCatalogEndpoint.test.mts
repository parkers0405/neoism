import { test } from "node:test";
import assert from "node:assert/strict";
import { ProtocolClient } from "../workspace/ProtocolClient.ts";
import { TerminalPanel } from "./TerminalPanel.ts";

// Exercise the real host methods without constructing a DOM/canvas or WASM.
function fixture() {
  const client = new ProtocolClient({
    url: "wss://daemon/session?token=url-secret",
    authToken: "auth-secret",
    pairingToken: "pairing-secret",
  });
  const bindings: string[] = [];
  const attaches: Array<string | null> = [];
  let root = "/project-a";
  const panel = Object.create(TerminalPanel.prototype) as any;
  Object.assign(panel, {
    options: { client, activeWorkspaceId: "workspace-a" },
    presenceIdentity: { displayName: "Browser" },
    agentLastAttachAt: 0,
    wasmAdapter: {
      agentSetCatalogEndpoint: (identity: string) => bindings.push(identity),
      fileTreeWorkspaceRoot: () => root,
      agentAttach: (directory: string | null) => {
        assert.ok(bindings.length, "catalog must be bound before attach");
        attaches.push(directory);
      },
    },
    ensureNotesSidebarSnapshot() {},
  });
  return { panel, client, bindings, attaches, setRoot(value: string) { root = value; } };
}

test("host binds catalog before attach and execution root changes retain its identity", () => {
  const { panel, bindings, attaches, setRoot } = fixture();
  panel.ensureNeoismAgentAttached();
  const identity = bindings.at(-1);
  setRoot("/project-b");
  panel.agentLastAttachAt = 0;
  panel.ensureNeoismAgentAttached();
  assert.equal(bindings.at(-1), identity);
  // The root still goes to execution attach, not into catalog cache scope.
  assert.deepEqual(attaches, ["/project-a", "/project-b"]);
  assert.ok(!identity!.includes("project-a"));
  assert.ok(!identity!.includes("secret"));
});

test("workspace authorization context is rebound immediately, even during attach debounce", () => {
  const { panel, bindings, attaches } = fixture();
  panel.ensureNeoismAgentAttached();
  const initial = bindings.at(-1);
  panel.setShareWorkspaceId("workspace-b");
  assert.notEqual(bindings.at(-1), initial);
  const changed = bindings.at(-1);
  panel.ensureNeoismAgentAttached();
  assert.equal(bindings.at(-1), changed);
  assert.equal(attaches.length, 1, "normal attach debounce remains in place");
  panel.setShareWorkspaceId("workspace-b");
  assert.equal(bindings.at(-1), changed);
});

test("new authorized client at the same URL and changed authenticated host rebind the catalog", () => {
  const { panel, bindings } = fixture();
  panel.ensureNeoismAgentAttached();
  const initial = bindings.at(-1);
  const replacement = new ProtocolClient({
    url: "wss://daemon/session?token=url-secret",
    authToken: "different-auth",
    pairingToken: "different-pairing",
  });
  panel.options.client = replacement;
  panel.ensureNeoismAgentAttached();
  const replaced = bindings.at(-1);
  assert.notEqual(replaced, initial);
  (replacement as any).handleRawMessage(JSON.stringify({
    WorkspaceReply: {
      request_id: 1,
      message: { HelloAck: { accepted: true, connected_host_id: "new-host" } },
    },
  }));
  panel.syncAgentCatalogEndpoint();
  assert.notEqual(bindings.at(-1), replaced);
});

test("binding is replayed for a newly ready adapter rather than cached by JS", () => {
  const { panel, bindings } = fixture();
  panel.ensureNeoismAgentAttached();
  const replay: string[] = [];
  panel.wasmAdapter = { agentSetCatalogEndpoint: (identity: string) => replay.push(identity) };
  panel.syncAgentCatalogEndpoint();
  assert.deepEqual(replay, [bindings.at(-1)]);
});
