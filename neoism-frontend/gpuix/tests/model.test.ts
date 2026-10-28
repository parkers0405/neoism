import { afterEach, expect, test } from "bun:test";
import type { Server, ServerWebSocket } from "bun";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";

const resources: Array<() => void> = [];
afterEach(() => resources.splice(0).reverse().forEach((close) => close()));
async function until(predicate: () => boolean) {
  const deadline = Date.now() + 2000;
  while (!predicate()) { if (Date.now() > deadline) throw new Error("State did not converge"); await Bun.sleep(5); }
}
test("workspace switches discard stale replies; notes keep their own vault for reads and saves", async () => {
  const workspaces = [
    { id: "a", host_id: "host", title: "A", root_dir: "/project/a" },
    { id: "b", host_id: "host", title: "B", root_dir: "/project/b", notes_vault_dir: "/vault/b" },
  ];
  const writtenRoots: string[] = [];
  let ptySequence=0,activeWorkspaceId="a";
  let delayed: (() => void) | undefined;
  const server = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch: (request, server) => server.upgrade(request) ? undefined : new Response("", { status: 400 }),
    websocket: {
      message(socket, data) {
        const [family, envelope] = Object.entries(JSON.parse(String(data)))[0] as [string, { request_id: number; message: unknown; workspace_root?: string }];
        const reply = (message: unknown) => socket.send(JSON.stringify({ [family + "Reply"]: { request_id: envelope.request_id, message } }));
        const message = envelope.message as Record<string, unknown>;
        if (family === "Workspace") {
          if (message.Hello) reply({ HelloAck: { accepted: true } });
          else if (envelope.message === "RequestHostWorkspaceTree") reply({ HostWorkspaceTree: { hosts: [{ id: "host", label: "Host" }], workspaces, tabs: [] } });
          else if (message.ResolveInitialWorkspace) reply({ InitialWorkspaceResolved: { workspace: workspaces[0], reason: "host_active" } });
          else if (message.SubscribeWorkspace) {
            const selected = (message.SubscribeWorkspace as { workspace_id: string }).workspace_id;
            activeWorkspaceId=selected;
            reply({ HostWorkspaceUpserted: { workspace: workspaces.find((w) => w.id === selected) } });
          } else if(message.BindEditorSurface){
            const bound=message.BindEditorSurface as {surface_id:string;session_id:string;path:null};
            reply({EditorSurfaceChanged:{surface:{...bound,workspace_id:activeWorkspaceId,route_id:ptySequence}}});
          }
        } else if(family==="Pty"){
          if(message.CreatePty)reply({PtyCreated:{session_id:`fixture-main-${++ptySequence}`,presentation:"grid",shell:"/bin/bash"}});
          else reply("Ack");
        } else if (family === "Files") {
          if (message.ReadFile) { reply({ FileContent: { path: "note.md", bytes: [...new TextEncoder().encode("vault note")] } }); return; }
          if (message.WriteFile) { writtenRoots.push(envelope.workspace_root!); reply({ FileWritten: { path: "note.md" } }); return; }
          const payload = { DirListing: { path: "", entries: [{ name: envelope.workspace_root === "/project/a" ? "a.txt" : envelope.workspace_root === "/vault/b" ? "note.md" : "b.txt", is_dir: false }] } };
          if (envelope.workspace_root === "/project/a") delayed = () => reply(payload);
          else reply(payload);
        } else if (family === "Agent") reply({ ThreadList: { threads: [] } });
      },
    },
  });
  resources.push(() => server.stop(true));
  const config = { ...loadConfig(["--config", "/nonexistent/gpuix-test-config"], {}), endpoint: `ws://127.0.0.1:${server.port}/session`, authToken: undefined };
  const client = new DaemonConnection(config), model = new WorkspaceModel(client, config);
  resources.push(() => { model.dispose(); client.stop(); });
  model.start(); client.start();
  await until(() => !!delayed);
  expect(model.state.workspaces.map((w) => w.id)).toEqual(["a", "b"]);
  await model.activateWorkspace(workspaces[1]);
  delayed!(); await Bun.sleep(20);
  expect(model.state.activeWorkspace?.id).toBe("b");
  expect(model.state.directories[""].entries[0].name).toBe("b.txt");
  expect(model.state.workspaces.map((w) => w.id)).toEqual(["a", "b"]);
  await until(() => !!model.state.notesDirectories[""]);
  expect(model.state.notesDirectories[""].entries[0].name).toBe("note.md");
  await model.openFile("note.md", "/vault/b");
  const surface = model.state.surfaces.find((s) => s.id === model.state.activeSurfaceId)!;
  expect(surface.rootDir).toBe("/vault/b");
  const editor = model.getEditor(surface); expect(editor.text()).toBe("vault note");
  editor.call("paste", { text: "edited " });
  await model.saveFile(surface);
  expect(writtenRoots).toEqual(["/vault/b"]); expect(editor.frame.dirty).toBe(false);
  expect(ptySequence).toBe(0); // Welcome/workspace/file navigation never spawns a shell.
});
