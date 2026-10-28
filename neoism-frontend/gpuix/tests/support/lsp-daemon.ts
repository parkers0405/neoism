export function createLspDaemon() {
  const root = "/project/lsp", path = `${root}/main.ts`;
  const workspace = { id: "lsp", host_id: "host", title: "LSP", root_dir: root };
  const requests: Array<{ tag: string; body: Record<string, any> }> = [], writes: string[] = [];
  const source = "const value = 1;\nconsole.log(value);\n";
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
    fetch(request, server) {
      const url = new URL(request.url);
      if (url.pathname === "/session") return server.upgrade(request) ? undefined : new Response("", { status: 400 });
      if (url.pathname.endsWith("providers/configured")) return Response.json({ providers: [], default: {} });
      if (url.pathname.endsWith("agents") || url.pathname.endsWith("skills")) return Response.json([]);
      return new Response("Not found", { status: 404 });
    },
    websocket: { message(socket, data) {
      const [family, envelope] = Object.entries(JSON.parse(String(data)))[0] as [string, { request_id: number; message: any }];
      const tag = typeof envelope.message === "string" ? envelope.message : Object.keys(envelope.message)[0];
      const body = typeof envelope.message === "string" ? {} : envelope.message[tag];
      requests.push({ tag, body });
      const reply = (message: unknown) => socket.send(JSON.stringify({ [family+"Reply"]: { request_id: envelope.request_id, message } }));
      const scoped = (message: Record<string, any>) => { const key = Object.keys(message)[0]; reply({ [key]: { ...message[key], surface_id: body.surface_id } }); };
      if (family === "Workspace") {
        if (tag === "Hello") reply({ HelloAck: { accepted: true } });
        if (tag === "RequestHostWorkspaceTree") reply({ HostWorkspaceTree: { hosts: [{ id: "host", label: "Host" }], workspaces: [workspace], tabs: [] } });
        if (tag === "ResolveInitialWorkspace") reply({ InitialWorkspaceResolved: { workspace, reason: "host_active" } });
        if (tag === "SubscribeWorkspace") reply({ HostWorkspaceUpserted: { workspace } });
      }
      if (family === "Files") {
        if (tag === "ListDir") reply({ DirListing: { path: "", entries: [{ name: "main.ts", is_dir: false }] } });
        if (tag === "ReadFile") reply({ FileContent: { path: "main.ts", bytes: [...new TextEncoder().encode(source)] } });
        if (tag === "WriteFile") { writes.push(new TextDecoder().decode(Uint8Array.from(body.bytes))); reply({ FileWritten: { path: "main.ts" } }); }
      }
      if (family === "Agent" && tag === "ListThreads") reply({ ThreadList: { threads: [] } });
      if (family !== "Editor") return;
      if (tag === "OpenBuffer") scoped({ Diagnostics: { file_path: path, error: 1, items: [{ severity: "Error", message: "Fixture diagnostic", line: 0, col: 0, end_line: 0, end_col: 5, lnum: 1 }] } });
      if (tag === "LspQueryAt") {
        const base = { seq: body.seq, action: body.action, root };
        if (body.action === "completion") scoped({ LspCompletions: { seq: body.seq, items: [{ server_id: "fixture", file_path: path, document_revision: "revision", label: "done", kind: "function", detail: "Fixture function", documentation: "Completion docs", insert_text: "done", payload: { insertTextFormat: 1 } }] } });
        else if (body.action === "hover" || body.action === "signature_help") scoped({ LspHoverResult: { seq: body.seq, line: body.line, character: body.character, contents: "### Fixture hover\n\n```ts\nconst value: number\n```" } });
        else if (body.action === "definition") scoped({ LspQueryResult: { ...base, locations: [{ uri: `file://${path}`, line: 1, character: 8 }] } });
        else if (body.action === "references") scoped({ LspQueryResult: { ...base, references: [{ path: "main.ts", line: 2, column: 8, text: "console.log(value);" }] } });
        else if (body.action === "code_actions") scoped({ LspQueryResult: { ...base, code_actions: [{ server_id: "fixture", file_path: path, document_revision: "revision", title: "Apply fix", kind: "quickfix", payload: { title: "Apply fix", edit: {} } }] } });
        else if (body.action === "rename" || body.action === "format") scoped({ LspQueryResult: { ...base, title: body.action, edits: [{ path, edits: [{ start_line: 0, start_col: 0, end_line: 0, end_col: 4, new_text: body.action === "rename" ? body.text : "FORM" }] }] } });
      }
      if (tag === "ApplyLspCodeActionAt") scoped({ LspQueryResult: { seq: body.seq, action: "code_actions", root, title: "Apply fix", edits: [{ path, edits: [{ start_line: 0, start_col: 0, end_line: 0, end_col: 4, new_text: "FIXD" }] }] } });
    } },
  });
  return { endpoint: `ws://127.0.0.1:${server.port}/session`, root, path, requests, writes, stop: () => server.stop(true) };
}
