import { afterEach, expect, test } from "bun:test";
import { DatabaseController } from "../src/database/DatabaseController";
import { DaemonConnection } from "../src/daemon/client";
import { parseCell } from "../src/database/types";
import type { DatabaseReply, DatabaseRequest, DatabaseValue } from "../src/database/protocol";

const cleanups: (() => void)[] = [];
afterEach(() => { for (const close of cleanups.splice(0).reverse()) close(); });
const integer = (value: string): DatabaseValue => ({ kind: "integer", value });
const text = (value: string): DatabaseValue => ({ kind: "text", value });
const schema = { Schema: { objects: ["fruit", "other"].map(name => ({ name, kind: "table" as const, sql: "", columns: [
  { name: "id", data_type: "INTEGER", primary_key: 1, not_null: false, default_value: null },
  { name: "name", data_type: "TEXT", primary_key: 0, not_null: false, default_value: "'apple'" },
], indexes: [], triggers: [] })) } };

async function fixture(options: { failApply?: string; failBrowse?: boolean; removeOther?: boolean; total?: number } = {}) {
  const requests: { request: DatabaseRequest; root?: string }[] = [];
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
    fetch(request, server) { return server.upgrade(request) ? undefined : new Response("", { status: 400 }); },
    websocket: { message(socket, raw) {
      const envelope = JSON.parse(String(raw));
      if (envelope.Workspace?.message?.Hello) { socket.send(JSON.stringify({ WorkspaceReply: { request_id: 0, message: { HelloAck: { accepted: true } } } })); return; }
      const message = envelope.Database;
      if (!message) return;
      const request = message.message as DatabaseRequest;
      requests.push({ request, root: message.workspace_root });
      let reply: DatabaseReply;
      if ("Schema" in request) reply = { Schema: { objects: schema.Schema.objects.filter(object => !options.removeOther || object.name !== "other") } };
      else if ("Browse" in request) reply = options.failBrowse ? { Error: { message: "Could not read table" } } : { Rows: {
        columns: ["id", "name"], rows: [[integer("9007199254740993"), text("apple")]],
        identities: [{ id: integer("9007199254740993") }], total: options.total ?? 1, editable: true,
      } };
      else if ("Values" in request) reply = { Values: { values: [{ value: text("apple"), count: 1 }], truncated: false } };
      else reply = options.failApply ? { Error: { message: options.failApply } } : { Applied: { affected: request.Apply.changes.length } };
      socket.send(JSON.stringify({ DatabaseReply: { request_id: message.request_id, message: reply } }));
    } },
  });
  const client = new DaemonConnection({ endpoint: `ws://127.0.0.1:${server.port}/session` });
  const controller = new DatabaseController(client, "/workspace", "fruit.db");
  cleanups.push(() => server.stop(true), () => client.stop(), () => controller.dispose());
  client.start();
  const deadline = Date.now() + 2000;
  while (client.state.phase !== "connected") { if (Date.now() > deadline) throw new Error("Connection did not start"); await Bun.sleep(5); }
  await controller.initialize();
  return { controller, requests, options };
}

test("staged CRUD uses the original identity and preserves NULL, empty text, defaults and 64-bit integers", async () => {
  const { controller: db, requests } = await fixture();
  const key = db.state.rows[0]!.key;
  db.editCell(key, 0, integer("42"));
  db.editCell(key, 1, text(""));
  db.addRow();
  const added = db.state.rows.at(-1)!;
  db.editCell(added.key, 1, { kind: "null" });
  expect(db.state.pending).toBe(2);
  expect(requests.filter(entry => "Apply" in entry.request)).toHaveLength(0);
  await db.selectTable("other");
  expect(db.state.table).toBe("fruit");
  expect(db.state.error).toContain("Apply or discard");
  await db.apply();
  const apply = requests.find(entry => "Apply" in entry.request)!;
  expect(apply.root).toBe("/workspace");
  expect(apply.request).toEqual({ Apply: { path: "fruit.db", table: "fruit", changes: [
    { kind: "update", identity: { id: integer("9007199254740993") }, original: { id: integer("9007199254740993"), name: text("apple") }, values: { id: integer("42"), name: text("") } },
    { kind: "insert", values: { name: { kind: "null" } } },
  ] } });
  expect(db.state.pending).toBe(0);
  db.deleteRow(db.state.rows[0]!.key);
  expect(db.state.rows[0]!.kind).toBe("delete");
  db.discard();
  expect(db.state.rows[0]!.kind).toBe("existing");
  expect(db.state.pending).toBe(0);
});

test("column filter choices exclude their own filter while retaining stacked filters", async () => {
  const { controller: db, requests } = await fixture();
  await db.setFilter("id", [integer("9007199254740993")]);
  await db.setFilter("name", [text("apple")]);
  await db.loadValues("name");
  expect(requests.at(-1)!.request).toEqual({ Values: { path: "fruit.db", table: "fruit", column: "name", filters: [{ column: "id", values: [integer("9007199254740993")] }] } });
  await db.setFilter("name", []);
  const browse = requests.at(-1)!.request;
  expect("Browse" in browse && browse.Browse.filters.find(filter => filter.column === "name")?.values).toEqual([]);
  await db.setFilter("name", undefined);
  expect(db.state.filters.name).toBeUndefined();
});

test("pagination fetches only the selected page and protects staged edits when changing page size", async () => {
  const {controller:db,requests}=await fixture({total:125});
  expect(db.state.limit).toBe(50);
  await db.page(1);
  expect(requests.at(-1)!.request).toMatchObject({Browse:{offset:50,limit:50}});
  await db.setPageSize(25);
  expect(requests.at(-1)!.request).toMatchObject({Browse:{offset:0,limit:25}});
  await db.page(4);
  expect(db.state.offset).toBe(100);
  const count=requests.length;
  await db.page(1);
  expect(requests).toHaveLength(count);
  db.editCell(db.state.rows[0]!.key,1,text("changed"));
  await db.setPageSize(100);
  expect(db.state.pending).toBe(1);
  expect(db.state.limit).toBe(25);
  expect(db.state.error).toContain("Apply or discard");
  db.discard();
  await db.setPageSize(200);
  expect(requests.at(-1)!.request).toMatchObject({Browse:{offset:0,limit:200}});
});

test("failed navigation never exposes the previous table's rows under the new table name", async () => {
  const { controller: db, options } = await fixture();
  options.failBrowse = true;
  await db.selectTable("other");
  expect(db.state.table).toBe("other");
  expect(db.state.rows).toEqual([]);
  expect(db.state.editable).toBe(false);
  expect(db.state.error).toContain("Could not read table");
});

test("failed mutations retain the draft and unconfirmed Apply cannot be blindly retried", async () => {
  const { controller: db, requests, options } = await fixture({ failApply: "UNIQUE constraint failed" });
  db.editCell(db.state.rows[0]!.key, 1, text("new"));
  await db.apply();
  expect(db.state.pending).toBe(1);
  expect(db.state.error).toContain("UNIQUE constraint");
  options.failApply = "Database request timed out";
  await db.apply();
  expect(db.state.error).toContain("may have committed");
  await db.apply();
  expect(requests.filter(entry => "Apply" in entry.request)).toHaveLength(2);
});

test("cell input validates numeric values without rounding SQLite integers", () => {
  expect(parseCell("9007199254740993", "integer")).toEqual(integer("9007199254740993"));
  expect(parseCell("", "text")).toEqual(text(""));
  expect(parseCell("ignored", "null")).toEqual({ kind: "null" });
  expect(() => parseCell("9223372036854775808", "integer")).toThrow("64-bit");
  expect(() => parseCell("Infinity", "real")).toThrow("finite");
});

test("the shared schema sidebar does not browse rows and a table tab cannot fall back to another table", async () => {
  const { controller: existing, requests, options } = await fixture();
  const sidebar = new DatabaseController(existing.client, "/workspace", "fruit.db", undefined, null);
  const before = requests.length;
  await Promise.all([sidebar.initialize(), sidebar.initialize()]);
  expect(requests.length - before).toBe(1);
  expect(sidebar.state.objects).toHaveLength(2);
  expect(sidebar.state.rows).toEqual([]);
  const tab = new DatabaseController(existing.client, "/workspace", "fruit.db", undefined, "other");
  await tab.initialize();
  expect(tab.state.table).toBe("other");
  options.removeOther = true;
  await tab.refresh();
  expect(tab.state.table).toBe("other");
  expect(tab.state.editable).toBe(false);
  expect(tab.state.error).toContain("no longer exists");
  sidebar.dispose(); tab.dispose();
});