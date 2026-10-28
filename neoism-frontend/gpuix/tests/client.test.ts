import { afterEach, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import type WebSocket from "ws";
import { Effect } from "effect";
import { DaemonConnection } from "../src/daemon/client";

class FakeSocket extends EventEmitter {
  readyState = 1;
  sent: Record<string, { request_id: number; message: unknown }>[] = [];
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close() { this.readyState = 3; this.emit("close"); }
  terminate() { this.close(); }
  reply(family: string, requestId: number, message: unknown) { this.emit("message", Buffer.from(JSON.stringify({ [family + "Reply"]: { request_id: requestId, message } }))); }
}
const clients: DaemonConnection[] = [];
afterEach(() => clients.splice(0).forEach((client) => client.stop()));
const setup = () => {
  const sockets: FakeSocket[] = [];
  const client = new DaemonConnection({ endpoint: "ws://localhost/session" }, () => {
    const socket = new FakeSocket(); sockets.push(socket); return socket as unknown as WebSocket;
  });
  clients.push(client); client.start();
  return { client, sockets };
};
test("Hello precedes all service work; pre-auth pushes are ignored", async () => {
  const { client, sockets } = setup(), socket = sockets[0];
  const received: string[] = []; client.subscribe((event) => received.push(event.family));
  socket.emit("open");
  expect(socket.sent[0]).toEqual({ Workspace: { request_id: 0, message: { Hello: { token: null, client_name: "neoism-gpuix" } } } });
  expect(() => client.send("Files", { ListDir: { path: "" } })).toThrow();
  socket.reply("Files", 0, { DirListing: { path: "", entries: [] } }); expect(received).toEqual([]);
  socket.reply("Workspace", 0, { HelloAck: { accepted: true } });
  expect(client.state.phase).toBe("connected");
});
test("correlation includes the reply family; disposal fails pending work", async () => {
  const { client, sockets } = setup(), socket = sockets[0];
  socket.emit("open"); socket.reply("Workspace", 0, { HelloAck: { accepted: true } });
  const pending = Effect.runPromise(client.request("Files", { ListDir: { path: "src" } }));
  const requestId = socket.sent.at(-1)!.Files.request_id;
  socket.reply("Agent", requestId, { Error: { message: "wrong family" } });
  socket.reply("Files", requestId, { DirListing: { path: "src", entries: [] } });
  expect(await pending).toEqual({ DirListing: { path: "src", entries: [] } });
  const disconnected = Effect.runPromise(client.request("Files", { ReadFile: { path: "README.md" } })).catch((error) => error);
  client.stop(); expect(String(await disconnected)).toContain("Disconnected");
});
test("a rejected handshake stops reconnects and invalidates pending state", () => {
  const { client, sockets } = setup(), socket = sockets[0];
  socket.emit("open"); socket.reply("Workspace", 0, { HelloAck: { accepted: false, reason: "invalid token" } });
  expect(client.state).toMatchObject({ phase: "auth-rejected", reason: "invalid token" });
  socket.emit("close"); expect(client.state.phase).toBe("auth-rejected");
});
test("events from a replaced socket cannot hydrate the new connection", () => {
  const { client, sockets } = setup();
  client.reconnect(); sockets[0].emit("open"); sockets[0].reply("Workspace", 0, { HelloAck: { accepted: true } });
  expect(client.state.phase).toBe("connecting");
  sockets[1].emit("open"); sockets[1].reply("Workspace", 0, { HelloAck: { accepted: true } });
  expect(client.state.phase).toBe("connected");
});
test("search progress and late outer envelope ids cannot resolve the wrong concurrent search", async () => {
  const { client, sockets } = setup(), socket = sockets[0];
  socket.emit("open"); socket.reply("Workspace", 0, { HelloAck: { accepted: true } });
  const first = Effect.runPromise(client.request("Search", { SearchFiles: { req_id: 0, cwd: "", query: "one", mode: "Fuzzy" } }, "/project/one"));
  const a = socket.sent.at(-1)!.Search.request_id;
  const second = Effect.runPromise(client.request("Search", { SearchFiles: { req_id: 0, cwd: "", query: "two", mode: "Fuzzy" } }, "/project/two"));
  const b = socket.sent.at(-1)!.Search.request_id;
  expect(socket.sent.at(-1)!.Search.message).toMatchObject({ SearchFiles: { req_id: b } });
  socket.reply("Search", b, { SearchProgress: { req_id: a, found_so_far: 4 } });
  socket.reply("Search", b, { SearchFilesResult: { req_id: a, hits: [{ score: 1, path: "one.ts" }] } });
  socket.reply("Search", b, { SearchFilesResult: { req_id: b, hits: [{ score: 1, path: "two.ts" }] } });
  expect(await first).toMatchObject({ SearchFilesResult: { hits: [{ path: "one.ts" }] } });
  expect(await second).toMatchObject({ SearchFilesResult: { hits: [{ path: "two.ts" }] } });
});

test("connection failures retain actual transport codes and frame metadata while redacting credentials",()=>{
 const socket=new FakeSocket(),failures:unknown[]=[],client=new DaemonConnection({endpoint:"ws://localhost/session",authToken:"private-upgrade-secret",pairingToken:"private-pairing-secret"},()=>socket as unknown as WebSocket,failure=>failures.push(failure));clients.push(client);client.start();socket.emit("open");socket.reply("Workspace",0,{HelloAck:{accepted:true}});
 socket.reply("Agent",7,{HistoryChunk:{messages:[],session_id:"fixture"}});
 const error=Object.assign(new Error("Cannot read ?token=private-upgrade-secret using private-pairing-secret"),{code:"ERR_WS_UNSUPPORTED_MESSAGE_LENGTH"});socket.emit("error",error);
 expect(client.state.phase).toBe("waiting");expect(client.state.reason).toContain("ERR_WS_UNSUPPORTED_MESSAGE_LENGTH");expect(JSON.stringify(failures)).not.toContain("private-upgrade-secret");expect(JSON.stringify(failures)).not.toContain("private-pairing-secret");
 expect(failures).toMatchObject([{attempt:1,lastMessage:{family:"Agent",kind:"HistoryChunk"}}]);expect((failures[0] as {lastMessage:{bytes:number}}).lastMessage.bytes).toBeGreaterThan(20);
});
test("server close codes remain diagnostic and stale close events cannot fail the next connection",()=>{
 const sockets:FakeSocket[]=[],failures:unknown[]=[],client=new DaemonConnection({endpoint:"ws://localhost/session"},()=>{const socket=new FakeSocket();sockets.push(socket);return socket as unknown as WebSocket;},failure=>failures.push(failure));clients.push(client);client.start();sockets[0].emit("open");sockets[0].reply("Workspace",0,{HelloAck:{accepted:true}});sockets[0].emit("close",1011);
 expect(client.state.reason).toBe("Daemon disconnected (WebSocket 1011)");expect(failures).toHaveLength(1);client.reconnect();sockets[0].emit("close",1006);sockets[1].emit("open");sockets[1].reply("Workspace",0,{HelloAck:{accepted:true}});expect(client.state.phase).toBe("connected");expect(failures).toHaveLength(1);
});
