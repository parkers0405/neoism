import { afterEach, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import { Effect } from "effect";
import WebSocket from "ws";
import {
  DaemonConnection, DaemonError, Daemon, daemonLayer,
  parseEnvelope, expectVariant, WorkspaceSchema,
  type WorkspaceClientMessage,
} from "../src";

class FakeSocket extends EventEmitter {
  readyState: number = WebSocket.OPEN;
  sent: Record<string, { request_id: number; message: unknown; workspace_root?: string }>[] = [];
  send(raw: string) { this.sent.push(JSON.parse(raw)); }
  close() { this.readyState = WebSocket.CLOSED; this.emit("close"); }
  terminate() { this.close(); }
  reply(family: string, requestId: number, message: unknown) {
    this.emit("message", Buffer.from(JSON.stringify({ [`${family}Reply`]: { request_id: requestId, message } })));
  }
}
const clients: DaemonConnection[] = [];
afterEach(() => { for (const client of clients.splice(0)) client.stop(); });
function setup(clientName?: string, hooks?: ConstructorParameters<typeof DaemonConnection>[3]) {
  const socket = new FakeSocket();
  const client = new DaemonConnection({ endpoint: "ws://localhost/session", clientName, pairingToken: "paired" },
    () => socket as unknown as WebSocket, () => {}, hooks);
  clients.push(client);
  client.start(); socket.emit("open");
  return { client, socket };
}

test("standalone transport keeps the legacy constructor and supports the TUI identity", () => {
  const gui = setup();
  expect(gui.socket.sent[0]).toEqual({ Workspace: { request_id: 0, message: { Hello: { token: "paired", client_name: "neoism-gpuix" } } } });
  const { client, socket } = setup("neoism-tui");
  expect(socket.sent[0].Workspace.message).toEqual({ Hello: { token: "paired", client_name: "neoism-tui" } });
  expect(() => client.send("Workspace", "RequestHostWorkspaceTree" satisfies WorkspaceClientMessage)).toThrow(DaemonError);
  socket.reply("Workspace", 0, { HelloAck: { accepted: true, connected_host_id: "host-a" } });
  expect(client.state).toMatchObject({ phase: "connected", connectedHostId: "host-a" });
});

test("profiling and host batching wrap receives, including invalid JSON", () => {
  const order: string[] = [], metrics: unknown[] = [];
  const { client, socket } = setup("neoism-tui", {
    profileStart: () => { order.push("start"); return 123; },
    profileEnd: (...args) => { order.push("end"); metrics.push(args); },
    batch: run => { order.push("batch"); run(); order.push("flush"); },
  });
  client.subscribe(() => order.push("event"));
  socket.reply("Workspace", 0, { HelloAck: { accepted: true } });
  expect(order).toEqual(["start", "batch", "event", "flush", "end"]);
  expect(metrics[0]).toEqual(["daemon.receive.Workspace", 123, expect.any(Number)]);
  socket.emit("message", Buffer.from("invalid JSON"));
  expect(metrics[1]).toEqual(["daemon.receive.invalid", 123, 0]);
  expect(client.state.phase).toBe("waiting");
});

test("workspace lifecycle capability is optional and resets on a legacy handshake",()=>{
  const {client,socket}=setup();
  socket.reply("Workspace",0,{HelloAck:{accepted:true,workspace_lifecycle:true}});
  expect(client.state.workspaceLifecycle).toBe(true);
  socket.reply("Workspace",0,{HelloAck:{accepted:true}});
  expect(client.state.phase).toBe("connected");expect(client.state.workspaceLifecycle).toBeUndefined();
});

test("requests preserve family correlation, search progress, root routing, and shutdown rejection", async () => {
  const { client, socket } = setup("neoism-tui");
  socket.reply("Workspace", 0, { HelloAck: { accepted: true } });
  const result = Effect.runPromise(client.request("Search", { SearchFiles: { req_id: 0, cwd: "", query: "index", mode: "Fuzzy" } }, "/project"));
  const request = socket.sent.at(-1)!.Search;
  expect(request.workspace_root).toBe("/project");
  expect(request.message).toMatchObject({ SearchFiles: { req_id: request.request_id } });
  socket.reply("Files", request.request_id, { Error: { message: "wrong family" } });
  socket.reply("Search", 999, { SearchProgress: { req_id: request.request_id, found_so_far: 1 } });
  socket.reply("Search", 999, { SearchFilesResult: { req_id: request.request_id, hits: [] } });
  expect(await result).toMatchObject({ SearchFilesResult: { hits: [] } });
  const pending = Effect.runPromise(client.request("Files", { ListDir: { path: "" } })).catch(String);
  client.stop();
  expect(await pending).toContain("Disconnected");
});

test("Effect service layer owns connection lifecycle", async () => {
  const { client } = setup();
  client.stop();
  const connection = await Effect.runPromise(Effect.scoped(Effect.provide(Effect.gen(function* () { return yield* Daemon; }), daemonLayer(client))));
  expect(connection).toBe(client);
  expect(client.state.phase).toBe("closed");
});

test("wire exports retain schema helpers and legacy PTY/search envelope decoding", () => {
  expect(WorkspaceSchema).toBeDefined();
  expect(parseEnvelope('{"PtyOutput":{"session_id":"pty-a","data":"hi"}}')).toEqual({ family: "Pty", requestId: 0, message: { PtyOutput: { session_id: "pty-a", data: "hi" } } });
  expect(parseEnvelope('{"SearchReply":{"request_id":3,"message":{"SearchProgress":{"req_id":7}}}}')?.requestId).toBe(7);
  expect(parseEnvelope('{"FilesReply":{"request_id":-1,"message":"x"}}')).toBeUndefined();
  expect(expectVariant({ DirListing: { entries: [] } }, "DirListing")).toEqual({ entries: [] });
  expect(() => expectVariant({ Error: { message: "denied" } }, "DirListing")).toThrow("denied");
});

test("real WebSocket transport separates upgrade credentials from paired Hello credentials", async () => {
  let upgradeUrl = "", hello: unknown;
  const server = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch(request, server) {
      const url = new URL(request.url);
      upgradeUrl = url.pathname + url.search;
      return server.upgrade(request) ? undefined : new Response("Upgrade required", { status: 400 });
    },
    websocket: {
      message(socket, raw) {
        hello = JSON.parse(raw.toString());
        socket.send(JSON.stringify({ WorkspaceReply: { request_id: 0, message: { HelloAck: { accepted: true } } } }));
      },
    },
  });
  const client = new DaemonConnection({ endpoint: `ws://127.0.0.1:${server.port}/session?existing=yes`, authToken: "upgrade-secret", pairingToken: "pair-secret", clientName: "neoism-tui" });
  clients.push(client);
  try {
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("Connection timed out")), 2000);
      client.subscribeStatus(state => { if (state.phase === "connected") { clearTimeout(timeout); resolve(); } });
      client.start();
    });
    expect(upgradeUrl).toBe("/session?existing=yes&token=upgrade-secret");
    expect(hello).toEqual({ Workspace: { request_id: 0, message: { Hello: { token: "pair-secret", client_name: "neoism-tui" } } } });
  } finally {
    client.stop();
    await server.stop(true);
  }
});