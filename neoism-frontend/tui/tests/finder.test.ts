import { afterEach, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import WebSocket from "ws";
import { DaemonConnection } from "@neoism/service-client/client";
import { TuiFinder, normalizeFinderResults } from "../src/finder";

class Socket extends EventEmitter {
  readyState: number = WebSocket.OPEN;
  sent: Array<Record<string, { request_id: number; workspace_root?: string; message: any }>> = [];
  send(raw: string) { this.sent.push(JSON.parse(raw)); }
  terminate() { this.readyState = WebSocket.CLOSED; this.emit("close"); }
  reply(message: unknown, requestId = 999) {
    this.emit("message", Buffer.from(JSON.stringify({ SearchReply: { request_id: requestId, message } })));
  }
  searches() { return this.sent.flatMap(frame => frame.Search && !frame.Search.message.CancelSearch ? [frame.Search] : []); }
}
const cleanup: Array<() => void> = [];
afterEach(() => { for (const fn of cleanup.splice(0)) fn(); });
async function until(condition: () => boolean) {
  const deadline = Date.now() + 2000;
  while (!condition()) {
    if (Date.now() >= deadline) throw new Error("Finder did not converge");
    await Bun.sleep(5);
  }
}
function fixture() {
  const socket = new Socket();
  const client = new DaemonConnection({ endpoint: "ws://localhost/isolated-finder", clientName: "neoism-tui" },
    () => socket as unknown as WebSocket, () => {});
  client.start();
  socket.emit("open");
  socket.emit("message", Buffer.from(JSON.stringify({ WorkspaceReply: { request_id: 0, message: { HelloAck: { accepted: true } } } })));
  const finder = new TuiFinder(client);
  cleanup.push(() => { finder.dispose(); client.stop(); });
  return { socket, client, finder };
}

test("files use daemon root routing, initial listing, native debounce and mode cycle", async () => {
  const { socket, finder } = fixture();
  finder.search("/project", "");
  await until(() => socket.searches().length === 1);
  const first = socket.searches()[0]!;
  expect(first.workspace_root).toBe("/project");
  expect(first.message).toEqual({ SearchFiles: { req_id: first.request_id, cwd: "", query: "", mode: "Fuzzy" } });
  socket.reply({ SearchFilesResult: { req_id: first.request_id, hits: [{ score: 2, path: "src/main.rs" }, { score: 1, path: "README.md" }] } });
  await until(() => !finder.state.loading);
  expect(finder.state.results).toEqual([{ path: "src/main.rs" }, { path: "README.md" }]);
  expect(finder.state.progress).toBeUndefined();
  finder.search("/project", "m");
  finder.search("/project", "ma");
  finder.search("/project", "main");
  await Bun.sleep(30);
  expect(socket.searches()).toHaveLength(1);
  await until(() => socket.searches().length === 2);
  expect(socket.searches()[1]!.message.SearchFiles.query).toBe("main");
  finder.cycle();
  await until(() => socket.searches().length === 3);
  expect(socket.searches()[2]!.message.SearchFiles.mode).toBe("Exact");
  finder.cycle();
  expect(finder.state.matchMode).toBe("Fuzzy");
});

test("grep retains GUI minimum policy, regex exception and byte coordinates", async () => {
  const { socket, finder } = fixture();
  for (const query of ["", "  ", "x", "😀"]) finder.search("/project", query, "grep");
  await Bun.sleep(240);
  expect(socket.searches()).toHaveLength(0);
  expect(finder.state.loading).toBe(false);
  finder.search("/project", "x", "grep");
  finder.cycle();
  finder.cycle();
  await until(() => socket.searches().length === 1);
  const request = socket.searches()[0]!;
  expect(request.message).toEqual({ SearchGrep: { req_id: request.request_id, cwd: "", query: "x", mode: "Regex", case_sensitive: null, file_patterns: [] } });
  socket.reply({ SearchGrepResult: { req_id: request.request_id, hits: [{ score: 0, path: "src/a.ts", line: 1, column: 3, text: "é x" }] } });
  await until(() => !finder.state.loading);
  expect(finder.state.results).toEqual([{ path: "src/a.ts", line: 1, col: 3, text: "é x" }]);
  finder.cycle();
  expect(finder.state.matchMode).toBe("Fuzzy");
  expect(finder.state.results).toEqual([]);
});

test("only correlated real progress updates counts; progress is not a final result", async () => {
  const { socket, client, finder } = fixture();
  finder.search("/project", "needle", "grep");
  await until(() => socket.searches().length === 1);
  const id = socket.searches()[0]!.request_id;
  // Another consumer sharing this connection must remain independent.
  const other = client.send("Search", { SearchFiles: { req_id: 0, cwd: "", query: "other", mode: "Fuzzy" } }, "/other");
  socket.reply({ SearchProgress: { req_id: other, found_so_far: 100 } });
  expect(finder.state.progress).toBeUndefined();
  socket.reply({ SearchProgress: { req_id: id, found_so_far: 4 } });
  expect(finder.state.progress).toBe(4);
  expect(finder.state.loading).toBe(true);
  expect(finder.state.results).toEqual([]);
  socket.reply({ SearchGrepResult: { req_id: id, hits: [] } });
  await until(() => !finder.state.loading);
  socket.reply({ SearchProgress: { req_id: id, found_so_far: 99 } });
  expect(finder.state.progress).toBe(4);
});

test("query/root/mode changes cancel the real request id and ignore stale results/progress", async () => {
  const { socket, finder } = fixture();
  finder.search("/old", "");
  await until(() => socket.searches().length === 1);
  const old = socket.searches()[0]!.request_id;
  finder.search("/new", "needle", "grep");
  await until(() => socket.sent.some(frame => frame.Search?.message.CancelSearch?.req_id === old));
  const cancel = socket.sent.find(frame => frame.Search?.message.CancelSearch?.req_id === old)!.Search!;
  expect(cancel.request_id).not.toBe(old);
  expect(cancel.workspace_root).toBe("/old");
  await until(() => socket.searches().length === 2);
  const current = socket.searches()[1]!.request_id;
  socket.reply({ SearchGrepResult: { req_id: current, hits: [{ score: 0, path: "new.ts", line: 12, column: 0, text: "needle" }] } });
  await until(() => !finder.state.loading);
  socket.reply({ SearchProgress: { req_id: old, found_so_far: 50 } });
  socket.reply({ SearchFilesResult: { req_id: old, hits: [{ path: "stale.ts", score: 100 }] } });
  socket.reply({ SearchError: { req_id: old, message: "cancelled" } });
  await Bun.sleep(10);
  expect(finder.state.results).toEqual([{ path: "new.ts", line: 12, col: 0, text: "needle" }]);
  expect(finder.state.error).toBeUndefined();
  expect(finder.state.progress).toBeUndefined();
});

test("errors surface, next query clears them, and close/dispose cancel timers and requests", async () => {
  const { socket, finder } = fixture();
  finder.search("/project", "");
  await until(() => socket.searches().length === 1);
  socket.reply({ SearchError: { req_id: socket.searches()[0]!.request_id, message: "Refine query" } });
  await until(() => !finder.state.loading);
  expect(finder.state.error).toContain("Refine query");
  finder.search("/project", "pending");
  expect(finder.state.error).toBeUndefined();
  finder.cancel();
  await Bun.sleep(100);
  expect(socket.searches()).toHaveLength(1);
  finder.search("/project", "");
  await until(() => socket.searches().length === 2);
  const id = socket.searches()[1]!.request_id;
  let notifications = 0;
  finder.subscribe(() => notifications++);
  finder.dispose(); finder.dispose();
  await until(() => socket.sent.some(frame => frame.Search?.message.CancelSearch?.req_id === id));
  socket.reply({ SearchFilesResult: { req_id: id, hits: [{ path: "late", score: 0 }] } });
  finder.search("/project", "ignored");
  expect(notifications).toBe(0);
  expect(finder.state.loading).toBe(false);
});

test("pure normalization preserves daemon ordering, paths, and optional coordinates", () => {
  expect(normalizeFinderResults({ SearchFilesResult: { hits: [
    { path: "C:\\WORK\\src\\a.ts" }, { path: "C:\\workspace\\b.ts" }, { path: "relative.ts" }, null,
  ] } }, "c:\\work", "files")).toEqual([{ path: "src/a.ts" }, { path: "C:/workspace/b.ts" }, { path: "relative.ts" }]);
  expect(normalizeFinderResults({ SearchGrepResult: { hits: [
    { path: "/project/a", line: 2, column: 0, text: "match" },
    { path: "invalid", line: 0, column: 1, text: "bad" },
  ] } }, "/project", "grep")).toEqual([{ path: "a", line: 2, col: 0, text: "match" }]);
  expect(() => normalizeFinderResults({ Wrong: {} }, "/project", "files")).toThrow();
});

test("offline sends and mid-search disconnects fail promptly instead of hanging", async () => {
  const { client, socket, finder } = fixture();
  finder.search("/project", "");
  await until(() => socket.searches().length === 1);
  client.stop();
  await until(() => !finder.state.loading);
  expect(finder.state.error).toContain("offline");
  finder.search("/project", "");
  await until(() => !finder.state.loading);
  expect(finder.state.error).toContain("offline");
  expect(socket.searches()).toHaveLength(1);
});