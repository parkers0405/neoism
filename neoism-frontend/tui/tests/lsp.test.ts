import { describe, expect, test } from "bun:test";
import { DaemonConnection, type ConnectionState, type EditorClientMessage, type WireEvent } from "@neoism/service-client";
import { TuiLsp, byteToUtf16, utf16ToByte, lspRangeToBytes, parseHover, parseLocations, parseCompletions, completionEdits, mutationEdits, flattenSnippet, type LspEvent } from "../src/lsp";

const doc = { id: "a", path: "/work/a.ts", root: "/work", text: "a😀é.foo()\nsecond" };
const cursor = { line: 0, col: 7 };
const range = (start: number, end: number) => ({ start: { line: 0, character: start }, end: { line: 0, character: end } });
class FixtureClient {
  state: ConnectionState = { phase: "connected", attempt: 0 };
  sent: Array<{ id: number; message: EditorClientMessage; root?: string }> = [];
  listeners = new Set<(event: WireEvent) => void>();
  statuses = new Set<(state: ConnectionState) => void>();
  send(_family: "Editor", message: EditorClientMessage, root?: string) { const id = this.sent.length + 1; this.sent.push({ id, message, root }); return id; }
  subscribe = (listener: (event: WireEvent) => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  subscribeStatus = (listener: (state: ConnectionState) => void) => { this.statuses.add(listener); listener(this.state); return () => { this.statuses.delete(listener); }; };
  status(phase: ConnectionState["phase"]) { this.state = { phase, attempt: 0 }; for (const listener of this.statuses) listener(this.state); }
  last() { return this.sent.at(-1)!; }
  body() { return Object.values(this.last().message)[0] as Record<string, unknown>; }
  reply(kind: string, body: Record<string, unknown>, id = this.last().id) {
    const event: WireEvent = { family: "Editor", requestId: id, message: { [kind]: body } };
    for (const listener of this.listeners) listener(event);
  }
  result(extra: Record<string, unknown> = {}) {
    const body = this.body(); this.reply("LspQueryResult", { seq: body.seq, action: body.action, surface_id: body.surface_id, ...extra });
  }
}

describe("LSP payload parsers", () => {
  test("UTF-16 versus UTF-8 is explicit, including astral characters and CRLF", () => {
    expect(utf16ToByte("a😀é", 4)).toBe(7);
    expect(byteToUtf16("a😀é", 7)).toBe(4);
    expect(utf16ToByte("a😀é", 2)).toBe(1);
    expect(byteToUtf16("a😀é", 3)).toBe(1);
    expect(lspRangeToBytes(range(3, 4), "a😀é\r\n")).toEqual({ start: { line: 0, col: 5 }, end: { line: 0, col: 7 } });
    expect(() => lspRangeToBytes({ start: { line: 9, character: 0 }, end: { line: 9, character: 1 } }, "x")).toThrow("outside supplied text");
  });
  test("hover strings, MarkupContent, MarkedString arrays and empty results", () => {
    expect(parseHover({ contents: [{ language: "ts", value: "const x: number" }, { kind: "markdown", value: "**docs**" }, "tail"] })?.text)
      .toBe("```ts\nconst x: number\n```\n\n**docs**\n\ntail");
    expect(parseHover({ contents: { kind: "plaintext", value: "hello\r\nworld" } })?.lines).toEqual(["hello", "world"]);
    expect(parseHover("\n ")).toBeNull(); expect(parseHover(null)).toBeNull();
  });
  test("Location and LocationLink use target text, not source text or fake byte columns", () => {
    const payload = [{ uri: "file:///work/a%20b.ts", range: range(3, 4) }, { targetUri: "file:///work/a%20b.ts", targetRange: range(0, 4), targetSelectionRange: range(4, 4) }];
    expect(parseLocations(payload, path => path === "/work/a b.ts" ? "a😀é" : undefined)).toEqual([
      { path: "/work/a b.ts", line: 0, col: 5 }, { path: "/work/a b.ts", line: 0, col: 7 },
    ]);
    expect(() => parseLocations(payload, () => undefined)).toThrow("Target text required");
    expect(parseLocations(null, () => undefined)).toEqual([]);
  });
  test("CompletionList, snippets, insert/replace ranges, extra edits and defaults", () => {
    const parsed = parseCompletions({ isIncomplete: true, items: [{ label: "foo", insertTextFormat: 2,
      documentation: { kind: "markdown", value: "docs" }, textEdit: { insert: range(3, 3), replace: range(3, 4), newText: "foo($1)" },
      additionalTextEdits: [{ range: range(0, 1), newText: "b" }] }] }, doc.text);
    expect(parsed.incomplete).toBe(true);
    expect(parsed.items[0]).toMatchObject({ label: "foo", insertText: "foo($1)", snippet: true, documentation: "docs",
      edit: { range: { start: { line: 0, col: 5 }, end: { line: 0, col: 7 } }, insertRange: { start: { line: 0, col: 5 }, end: { line: 0, col: 5 } } },
      additionalEdits: [{ range: { start: { line: 0, col: 0 }, end: { line: 0, col: 1 } }, text: "b" }] });
    expect(parseCompletions([{ label: "simple" }], "").items[0]?.insertText).toBe("simple");
    expect(parseCompletions({ itemDefaults: { editRange: range(3, 4), insertTextFormat: 2 }, items: [{ label: "foo", textEditText: "$1" }] }, doc.text).items[0])
      .toMatchObject({ insertText: "$1", snippet: true, edit: { range: { start: { line: 0, col: 5 }, end: { line: 0, col: 7 } } } });
  });
  test("daemon completions retain payload and do not double-convert engine-normalized byte edits", () => {
    const item = { file_path: doc.path, document_revision: "rev", label: "foo", kind: "function", insert_text: "foo", preselect: true,
      payload: { insertTextFormat: 2, textEdit: { range: range(5, 7), newText: "foo($1)" } } };
    const parsed = parseCompletions({ replace_prefix: "é", items: [item] }, doc.text, true);
    expect(parsed.replacePrefix).toBe("é"); expect(parsed.items[0]?.raw).toBe(item);
    expect(parsed.items[0]).toMatchObject({ insertText: "foo($1)", snippet: true, edit: { range: { start: { line: 0, col: 5 }, end: { line: 0, col: 7 } } } });
  });
});

describe("TuiLsp protocol lifecycle", () => {
  test("format queries use mutation consent, supplied text/open paths, and return byte edits", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    await expect(lsp.format(doc.id, undefined as never)).rejects.toMatchObject({ code: "unsafe" });
    const options = { allowDiskWrites: true as const, openPaths: ["/work/other.ts"] };
    const formatted = lsp.format(doc.id, undefined, options);
    expect(client.body()).toMatchObject({ action: "format", line: 0, character: 0, buffer_text: doc.text, open_paths: [doc.path, "/work/other.ts"] });
    const file = { path: doc.path, edits: [{ start_line: 0, start_col: 5, end_line: 0, end_col: 7, new_text: "é " }] };
    client.result({ edits: [file], title: "Format" });
    const result = await formatted;
    expect(result).toEqual({ edits: [file], appliedFiles: [], title: "Format", ranCommand: false });
    expect(mutationEdits(result.edits[0]!)).toEqual([{ start_line: 0, start_col: 5, end_line: 0, end_col: 7, text: "é " }]);
    const anchored = lsp.format(doc.id, cursor, options); expect(client.body().character).toBe(7); client.result(); await anchored;
    const abbreviated = lsp.format(doc.id, options); expect(client.body().character).toBe(0); client.result(); await abbreviated;
    lsp.dispose();
  });
  test("open/change/close use supplied text/root and unique surfaces; reconnect restores current snapshots", () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client);
    lsp.open(doc); const surface = client.body().surface_id as string;
    expect(client.last()).toMatchObject({ root: doc.root, message: { OpenBuffer: { path: doc.path, text: doc.text, surface_id: surface } } });
    lsp.update({ ...doc, text: "changed" }); expect(client.body().text).toBe("changed"); expect(client.body().surface_id).toBe(surface);
    lsp.update({ ...doc, text: "changed" }); expect(client.sent.length).toBe(2);
    client.status("waiting"); lsp.update({ ...doc, text: "offline edit" }); expect(client.sent.length).toBe(2);
    client.status("connected"); expect(client.body().text).toBe("offline edit");
    lsp.close(doc.id); expect(client.last().message).toEqual({ CloseLspBuffer: { surface_id: surface } });
    lsp.open(doc); expect(client.body().surface_id).not.toBe(surface);
    lsp.dispose(); expect(client.listeners.size).toBe(0); expect(client.statuses.size).toBe(0);
  });
  test("scoped batch diagnostics/status subscriptions ignore other documents and disposed controllers", () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client), events: LspEvent[] = [];
    const unsubscribe = lsp.subscribe(event => events.push(event)); lsp.open(doc); const surface = client.body().surface_id;
    const item = { severity: "Error" as const, message: "oops", source: null, line: 0, col: 7, end_line: 0, end_col: 8, lnum: 1 };
    client.reply("Diagnostics", { surface_id: "other", file_path: doc.path, items: [item] });
    client.reply("Diagnostics", { surface_id: surface, file_path: "/wrong", items: [item] });
    expect(events).toHaveLength(0);
    client.reply("Batch", { surface_id: surface, messages: [{ Diagnostics: { file_path: doc.path, items: [item] } }, { LspStatus: { state: "Active" } }] });
    expect(events[0]).toEqual({ kind: "diagnostics", documentId: doc.id, path: doc.path, items: [item] });
    expect(events[1]?.kind).toBe("status");
    client.reply("Diagnostics", { surface_id: surface, items: [] }); expect(events[2]).toMatchObject({ items: [] });
    unsubscribe(); lsp.dispose(); expect(events).toHaveLength(3);
  });
  test("queries carry UTF-8 cursor, text, identity; wrong request/seq/surface/action cannot resolve", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    const promise = lsp.hover(doc.id, cursor), request = client.last(), body = client.body();
    expect(body).toMatchObject({ path: doc.path, line: 0, character: 7, buffer_text: doc.text, action: "hover" });
    client.reply("LspHoverResult", { ...body, contents: "wrong", surface_id: "other" });
    client.reply("LspHoverResult", { ...body, contents: "wrong", seq: -1 });
    client.reply("LspHoverResult", { ...body, contents: "wrong" }, request.id + 100);
    client.reply("LspQueryResult", { ...body, action: "definition", contents: "wrong" });
    client.reply("LspHoverResult", { ...body, contents: "right" });
    expect(await promise).toEqual({ text: "right", lines: ["right"] });
    const signature = lsp.signature(doc.id, cursor); client.reply("LspHoverResult", { ...client.body(), contents: "f(a)\n▸ a" });
    expect((await signature)?.lines).toEqual(["f(a)", "▸ a"]);
    lsp.dispose();
  });
  test("normalized navigation, references, symbols, completion, and actions", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    const definition = lsp.definition(doc.id, cursor); client.result({ locations: [{ uri: "file:///work/a.ts", line: 0, character: 7 }] });
    expect(await definition).toEqual([{ path: doc.path, line: 0, col: 7 }]); // Already bytes: do NOT convert again.
    const references = lsp.references(doc.id, cursor); client.result({ references: [{ path: "a.ts", line: 2, column: 7, text: "reference" }] });
    expect(await references).toEqual([{ path: doc.path, line: 1, col: 7, text: "reference" }]);
    const symbols = lsp.symbols(doc.id); client.result({ symbols: [{ uri: doc.path, line: 0, character: 7, name: "foo", kind: "function", depth: 0 }] });
    expect((await symbols)[0]).toMatchObject({ name: "foo", col: 7, depth: 0 });
    const completion = lsp.completion(doc.id, cursor, "."); expect(client.body().text).toBe(".");
    client.reply("LspCompletions", { ...client.body(), replace_prefix: "", items: [{ label: "foo", insert_text: "foo", kind: "function" }] });
    expect((await completion).items[0]?.insertText).toBe("foo");
    const action = { server_id: "ts", file_path: doc.path, document_revision: "rev", title: "Fix", preferred: false, payload: { edit: {} } };
    const actions = lsp.codeActions(doc.id, cursor); client.result({ code_actions: [action] }); expect(await actions).toEqual([action]);
    lsp.dispose();
  });
  test("stale replies reject on supersession, edit, close, reused ID, and disconnect", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    const first = lsp.hover(doc.id, cursor).catch(error => error);
    const second = lsp.hover(doc.id, cursor).catch(error => error);
    lsp.update({ ...doc, text: doc.text + "!" }); expect(await first).toMatchObject({ code: "stale" }); expect(await second).toMatchObject({ code: "stale" });
    const third = lsp.definition(doc.id, cursor).catch(error => error);
    const old = client.body(); lsp.close(doc.id); lsp.open(doc); expect(await third).toMatchObject({ code: "closed" });
    const fourth = lsp.hover(doc.id, cursor).catch(error => error);
    client.reply("LspHoverResult", { ...old, contents: "old document" }); client.status("waiting"); expect(await fourth).toMatchObject({ code: "offline" });
    lsp.dispose();
  });
  test("errors, timeout and invalid byte positions are surfaced without output", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client, 10); lsp.open(doc);
    await expect(lsp.hover(doc.id, { line: 0, col: 2 })).rejects.toMatchObject({ code: "protocol" });
    const error = lsp.definition(doc.id, cursor).catch(error => error);
    client.reply("Error", { surface_id: client.body().surface_id, message: "unsupported" }); expect(await error).toMatchObject({ message: "unsupported" });
    await expect(lsp.hover(doc.id, cursor)).rejects.toMatchObject({ code: "timeout" });
    lsp.dispose();
    await expect(lsp.hover(doc.id, cursor)).rejects.toMatchObject({ code: "closed" });
  });
  test("independent document callers cannot consume each other's replies; changing path retires identity", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    const a = lsp.hover(doc.id, cursor), aRequest = client.last(), aBody = client.body();
    lsp.open({ ...doc, id: "b", path: "/work/b.ts" });
    const b = lsp.hover("b", cursor), bRequest = client.last(), bBody = client.body();
    client.reply("LspHoverResult", { ...bBody, contents: "b" }, aRequest.id); // Wrong caller, even with valid surface.
    client.reply("LspHoverResult", { ...aBody, contents: "a" }, aRequest.id);
    client.reply("LspHoverResult", { ...bBody, contents: "b" }, bRequest.id);
    expect((await a)?.text).toBe("a"); expect((await b)?.text).toBe("b");
    const pending = lsp.hover(doc.id, cursor).catch(error => error), oldSurface = client.body().surface_id;
    lsp.update({ ...doc, path: "/work/renamed.ts" });
    expect(await pending).toMatchObject({ code: "closed" }); expect(client.body().surface_id).not.toBe(oldSurface);
    expect(client.body().path).toBe("/work/renamed.ts");
    lsp.dispose();
  });
  test("rename/apply require consent, include complete open paths, and return edits without changing documents", async () => {
    const client = new FixtureClient(), lsp = new TuiLsp(client); lsp.open(doc);
    await expect(lsp.rename(doc.id, cursor, "bar", undefined as never)).rejects.toMatchObject({ code: "unsafe" });
    const options = { allowDiskWrites: true as const, openPaths: ["/work/other.ts"] };
    const renamed = lsp.rename(doc.id, cursor, "bar", options);
    expect(client.body()).toMatchObject({ action: "rename", text: "bar", open_paths: [doc.path, "/work/other.ts"] });
    const edits = [{ path: doc.path, edits: [{ start_line: 0, start_col: 7, end_line: 0, end_col: 8, new_text: "bar" }] }];
    client.result({ edits, applied_files: ["/work/closed.ts"], title: "Rename" });
    expect(await renamed).toEqual({ edits, appliedFiles: ["/work/closed.ts"], title: "Rename", ranCommand: false });
    const action = { server_id: "ts", file_path: doc.path, document_revision: "rev", title: "Fix", preferred: false, payload: {} };
    const applied = lsp.applyCodeAction(doc.id, cursor, action, options);
    expect(client.last().message).toHaveProperty("ApplyLspCodeActionAt");
    client.reply("LspQueryResult", { ...client.body(), action: "code_actions", ran_command: true }); expect((await applied).ranCommand).toBe(true);
    expect(client.sent.filter(row => typeof row.message === "object" && "OpenBuffer" in row.message)).toHaveLength(1);
    lsp.dispose();
  });
});

describe("completion edit conversion", () => {
  test("explicit Unicode replace range wins over prefix; additional edits are literal and byte-based", () => {
    const result = parseCompletions({ replace_prefix: "not a suffix", items: [{ label: "foo", insert_text: "ignored", payload: {
      insertTextFormat: 2, textEdit: { insert: range(5, 7), replace: range(5, 8), newText: "${1:é}($1)$0" },
      additionalTextEdits: [{ range: range(0, 1), newText: "$literal" }],
    } }] }, doc.text, true);
    expect(completionEdits(result, result.items[0]!, cursor, doc.text)).toEqual([
      { start_line: 0, start_col: 5, end_line: 0, end_col: 8, text: "é(é)" },
      { start_line: 0, start_col: 0, end_line: 0, end_col: 1, text: "$literal" },
    ]);
  });
  test("fallback prefix uses UTF-8 length and validates actual cursor suffix, including CRLF", () => {
    const text = "first\r\na😀é";
    const result = parseCompletions({ replace_prefix: "😀é", items: [{ label: "done", insert_text: "done" }] }, text, true);
    expect(completionEdits(result, result.items[0]!, { line: 1, col: 7 }, text)).toEqual([
      { start_line: 1, start_col: 1, end_line: 1, end_col: 7, text: "done" },
    ]);
    expect(() => completionEdits(result, result.items[0]!, { line: 1, col: 5 }, text)).toThrow("suffix");
    expect(() => completionEdits(result, result.items[0]!, { line: 1, col: 3 }, text)).toThrow("byte boundary");
    const empty = { ...result, replacePrefix: "" };
    expect(completionEdits(empty, empty.items[0]!, { line: 1, col: 7 }, text)[0]?.start_col).toBe(7);
  });
  test("snippet defaults, mirrors, choices, escapes, variables with defaults and nested placeholders", () => {
    expect(flattenSnippet("$1 ${1:arg} ${2:${3:inner}} $3 ${4|one\\,two,three|} \\$ \\} \\\\ $0 ${TM_FILENAME:file.ts}"))
      .toBe("arg arg inner inner one,two $ } \\  file.ts");
    expect(flattenSnippet("f($1, ${2})$0")).toBe("f(, )");
    for (const snippet of ["${1/regex/replace/}", "$TM_FILENAME", "${1:unclosed", "${1|unclosed", "${1:".repeat(40) + "x" + "}".repeat(40)]) {
      expect(() => flattenSnippet(snippet)).toThrow();
    }
  });
  test("reject commands, invalid or overlapping ranges rather than silently apply partial edits", () => {
    const result = parseCompletions([{ label: "run", command: { command: "server.command", title: "Run" } }], doc.text);
    expect(() => completionEdits(result, result.items[0]!, cursor, doc.text)).toThrow("commands are unsupported");
    const parsed = parseCompletions([{ label: "foo", textEdit: { range: range(3, 4), newText: "foo" }, additionalTextEdits: [{ range: range(3, 4), newText: "other" }] }], doc.text);
    expect(() => completionEdits(parsed, parsed.items[0]!, cursor, doc.text)).toThrow("overlap");
    const item = { ...parsed.items[0]!, additionalEdits: [], edit: { range: { start: { line: 0, col: 7 }, end: { line: 0, col: 5 } }, text: "foo" } };
    expect(() => completionEdits(parsed, item, cursor, doc.text)).toThrow("reversed");
    expect(() => completionEdits(parsed, { ...item, edit: { ...item.edit, range: { start: { line: 0, col: 2 }, end: { line: 0, col: 7 } } } }, cursor, doc.text)).toThrow("byte boundary");
  });
});

test("real shared DaemonConnection through isolated Bun websocket fixture", async () => {
  const received: Array<Record<string, any>> = [];
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch(request, server) { if (server.upgrade(request)) return; return new Response("websocket only", { status: 400 }); },
    websocket: { message(socket, raw) {
      const envelope = JSON.parse(String(raw)); received.push(envelope);
      if (envelope.Workspace?.message.Hello) socket.send(JSON.stringify({ WorkspaceReply: { request_id: 0, message: { HelloAck: { accepted: true } } } }));
      const editor = envelope.Editor; if (!editor) return;
      const open = editor.message.OpenBuffer;
      if (open) socket.send(JSON.stringify({ EditorReply: { request_id: editor.request_id, message: { Diagnostics: { surface_id: open.surface_id, file_path: open.path, items: [] } } } }));
      const query = editor.message.LspQueryAt;
      if (query) socket.send(JSON.stringify({ EditorReply: { request_id: editor.request_id, message: { LspHoverResult: { surface_id: query.surface_id, seq: query.seq, line: query.line, character: query.character, contents: "fixture hover" } } } }));
    } },
  });
  const client = new DaemonConnection({ endpoint: `ws://127.0.0.1:${server.port}`, clientName: "tui-lsp-test" }, undefined, () => {});
  const lsp = new TuiLsp(client, 1000);
  const waitFor = async (predicate: () => boolean) => { const end = Date.now() + 2000; while (!predicate()) { if (Date.now() > end) throw new Error("Fixture timed out"); await Bun.sleep(5); } };
  try {
    const events: LspEvent[] = []; lsp.subscribe(event => events.push(event)); lsp.open(doc); client.start();
    await waitFor(() => client.state.phase === "connected" && events.length > 0);
    expect(await lsp.hover(doc.id, cursor)).toEqual({ text: "fixture hover", lines: ["fixture hover"] });
    lsp.update({ ...doc, text: "live text" }); lsp.close(doc.id);
    await waitFor(() => received.some(row => row.Editor?.message.CloseLspBuffer));
    const messages = received.filter(row => row.Editor).map(row => row.Editor);
    expect(messages.map(row => Object.keys(row.message)[0])).toEqual(["OpenBuffer", "LspQueryAt", "OpenBuffer", "CloseLspBuffer"]);
    expect(messages.every(row => row.workspace_root === doc.root)).toBe(true);
    expect(messages[2]?.message.OpenBuffer.text).toBe("live text");
  } finally { lsp.dispose(); client.stop(); await server.stop(true); }
});