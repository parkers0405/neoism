import * as Y from "yjs";
import { loadTuiConfig } from "../../src/config";
import { TuiModel } from "../../src/model";

export async function until(predicate: () => boolean, reason = "Fixture state", timeout = 4000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(reason);
    await Bun.sleep(5);
  }
}

export const workspace = { id: "work", host_id: "local-host", title: "Neoism", root_dir: "/fixture/project" };
export const secondWorkspace = { ...workspace, id: "second", root_dir: "/fixture/second" };

/** Real WebSocket transport and native Yrs editors; Yjs is only the server peer. */
export function modelFixture(options: { workspaceId?: string; delayFiles?: boolean; binary?: boolean; holdSnapshots?: boolean; configHome?: string; rootEntries?: Array<{ name: string; is_dir: boolean }> } = {}) {
  const requests: Array<{ family: string; message: any; workspace_root?: string }> = [];
  const docs = new Map<string, Y.Doc>(), saved = new Map<string, string>();
  const sockets = new Set<any>(), held: Array<() => void> = [], timers: ReturnType<typeof setTimeout>[] = [];
  const reply = (socket: any, family: string, request_id: number, message: unknown) => socket.send(JSON.stringify({ [family]: { request_id, message } }));
  const snapshot = (socket: any, id: string, requestId: number) => {
    const doc = docs.get(id)!;
    reply(socket, "Crdt", requestId, { Snapshot: { buffer_id: id, update_v1: [...Y.encodeStateAsUpdate(doc)], state_vector_v1: [...Y.encodeStateVector(doc)] } });
  };
  const server = Bun.serve<{ ids: Set<string> }>({
    hostname: "127.0.0.1", port: 0,
    fetch(request, server) { return server.upgrade(request, { data: { ids: new Set<string>() } }) ? undefined : new Response("WebSocket required", { status: 400 }); },
    websocket: {
      open(socket) { sockets.add(socket); },
      close(socket) { sockets.delete(socket); },
      message(socket, raw) {
        const [family, packet] = Object.entries(JSON.parse(String(raw)))[0]! as [string, any];
        const { request_id: id, message, workspace_root } = packet;
        requests.push({ family, message, workspace_root });
        const respond = (value: unknown) => reply(socket, family, id, value);
        if (message.Hello) respond({ HelloAck: { accepted: true, connected_host_id: "local-host" } });
        else if (message === "RequestHostWorkspaceTree") respond({ HostWorkspaceTree: { hosts: [], workspaces: [{ ...workspace, id: "foreign", host_id: "remote" }, workspace, secondWorkspace], tabs: [] } });
        else if (message.ListDir) respond({ DirListing: { path: message.ListDir.path, entries: message.ListDir.path ? [{ name: "code.rs", is_dir: false }] : options.rootEntries || [{ name: "z.txt", is_dir: false }, { name: "src", is_dir: true }] } });
        else if (family === "Editor" && message.LspQueryAt) {
          const { seq, action, surface_id, path } = message.LspQueryAt;
          const common = { seq, surface_id, file_path: path };
          if (action === "hover") respond({ LspHoverResult: { ...common, contents: "Fixture hover documentation" } });
          else if (action === "completion") respond({ LspCompletions: { ...common, replace_prefix: "", items: [{ label: "fixtureCompletion", insert_text: "fixtureCompletion" }] } });
          else respond({ LspQueryResult: { ...common, action, locations: [{ host_path: "/fixture/project/src/definition.rs", line: 0, character: 3 }] } });
        }
        else if (message.ReadFile) {
          const path = message.ReadFile.path;
          const send = () => respond({ FileContent: { path, bytes: options.binary ? [0, 1] : [...new TextEncoder().encode(`content of ${path}`)] } });
          if (options.delayFiles) timers.push(setTimeout(send, path === "slow.txt" ? 50 : 5)); else send();
        } else if (family === "Crdt" && message.OpenBuffer) {
          const { buffer_id, initial_text } = message.OpenBuffer;
          if (!docs.has(buffer_id)) {
            const read = requests.filter(request => request.message.ReadFile && buffer_id.includes(request.message.ReadFile.path)).at(-1);
            const doc = new Y.Doc(); doc.getText("buffer").insert(0, initial_text ?? (read ? `content of ${read.message.ReadFile.path}` : "")); docs.set(buffer_id, doc);
          }
          socket.data.ids.add(buffer_id);
          const send = () => snapshot(socket, buffer_id, id);
          if (options.holdSnapshots) held.push(send); else send();
        } else if (message.RequestSnapshot) snapshot(socket, message.RequestSnapshot.buffer_id, id);
        else if (message.ApplySync) {
          const envelope = message.ApplySync.envelope;
          Y.applyUpdate(docs.get(envelope.buffer_id)!, Uint8Array.from(envelope.update_v1));
          for (const peer of sockets) if (peer.data.ids.has(envelope.buffer_id)) reply(peer, "Crdt", peer === socket ? id : 0, { Sync: { envelope } });
        } else if (message.SaveBuffer) {
          const buffer_id = message.SaveBuffer.buffer_id, text = docs.get(buffer_id)!.getText("buffer").toString();
          saved.set(buffer_id, text);
          for (const peer of sockets) if (peer.data.ids.has(buffer_id)) reply(peer, "Crdt", peer === socket ? id : 0, { Saved: { buffer_id, bytes_written: Buffer.byteLength(text) } });
        }
      },
    },
  });
  const models: TuiModel[] = [];
  const make = () => {
    const config = loadTuiConfig(["--daemon", `ws://127.0.0.1:${server.port}/session`, ...(options.workspaceId ? ["--workspace", options.workspaceId] : [])], { NEOISM_CONFIG_HOME: options.configHome ?? `/nonexistent/tui-test-${crypto.randomUUID()}` });
    const model = new TuiModel(config); models.push(model); model.start(); return model;
  };
  return { model: make(), make, requests, docs, saved,
    releaseSnapshots() { options.holdSnapshots = false; for (const send of held.splice(0)) send(); },
    dispose() { for (const model of models) model.stop(); for (const timer of timers) clearTimeout(timer); server.stop(true); for (const doc of docs.values()) doc.destroy(); },
  };
}