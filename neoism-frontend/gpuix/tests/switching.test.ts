import { afterEach, expect, test } from "bun:test";
import { Effect } from "effect";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";

const cleanup: Array<() => void> = [];
afterEach(() => cleanup.splice(0).reverse().forEach(close => close()));
const a = { id: "a", host_id: "host", title: "A", root_dir: "/a" }, b = { ...a, id: "b", title: "B", root_dir: "/b" };
async function until(predicate: () => boolean) {
  const end = performance.now() + 1000;
  while (!predicate()) { if (performance.now() > end) throw new Error("Fixture did not converge"); await Bun.sleep(1); }
}
function fixture(options: { gateSubscribe?: boolean; gateReads?: boolean } = {}) {
  const config = loadConfig(["--config", "/nonexistent/switching-test"], {}), client = new DaemonConnection(config);
  const requests: Array<{ family: string; message: any; root?: string }> = [], subscriptions: Array<() => void> = [], reads: Array<() => void> = [];
  client.request = ((family: string, message: any, root?: string) => Effect.promise(async () => {
    requests.push({ family, message, root });
    if (message.SubscribeWorkspace) {
      if (options.gateSubscribe) await new Promise<void>(resolve => subscriptions.push(resolve));
      return { HostWorkspaceUpserted: { workspace: message.SubscribeWorkspace.workspace_id === "a" ? a : b } };
    }
    if (message.ListDir) return { DirListing: { path: message.ListDir.path, entries: [{ name: root === "/a" ? "a.txt" : "b.txt", is_dir: false }] } };
    if (message.ReadFile) {
      if (options.gateReads) await new Promise<void>(resolve => reads.push(resolve));
      return { FileContent: { path: message.ReadFile.path, bytes: [...new TextEncoder().encode("one\ntwo\nthree")] } };
    }
    throw new Error("Unexpected fixture request");
  })) as typeof client.request;
  client.send = (() => 1) as typeof client.send;
  const model = new WorkspaceModel(client, config); model.refreshCatalogs = async () => {};
  model.update({ workspaces: [a, b] }); cleanup.push(() => model.dispose());
  return { model, requests, subscriptions, reads };
}

test("directory loading starts before subscription acknowledgement and warm workspace navigation is immediate", async () => {
  const { model, requests, subscriptions } = fixture({ gateSubscribe: true });
  const first = model.activateWorkspace(a);
  await until(() => !!model.state.directories[""]);
  expect(subscriptions).toHaveLength(1); expect(model.state.directories[""].entries[0].name).toBe("a.txt");
  subscriptions.shift()!(); await first;
  model.toggleDirectory("folder"); await until(() => !!model.state.directories.folder);
  model.update({ threads: [{ session_id: "thread-a", title: "A thread", directory: "/a" }] });
  const cached = model.state.directories;
  const second = model.activateWorkspace(b); await until(() => subscriptions.length === 1); subscriptions.shift()!(); await second;
  const returned = model.activateWorkspace(a);
  // Assert before awaiting any subscription/RPC: no empty tree or loading flash.
  expect(model.state.directories).toBe(cached); expect(model.state.expanded.has("folder")).toBe(true);
  expect(model.state.threads[0].session_id).toBe("thread-a"); expect(model.state.loading).toBe(false);
  await until(() => subscriptions.length === 1); subscriptions.shift()!(); await returned;
  const count = requests.length; await model.activateWorkspace(a); expect(requests.length).toBe(count);
});

test("duplicate file clicks coalesce reads, retain scoped results across switches, and keep dirty views", async () => {
  const { model, requests, reads } = fixture({ gateReads: true }); await model.activateWorkspace(a);
  const first = model.openFile("a.txt"), repeated = model.openFile("a.txt");
  await until(() => reads.length === 1);
  expect(requests.filter(r => r.message.ReadFile)).toHaveLength(1);
  await model.activateWorkspace(b); reads.shift()!(); await Promise.all([first, repeated]);
  expect(model.state.activeWorkspace?.id).toBe("b"); expect(model.state.files["file:a:a.txt"]).toBe("one\ntwo\nthree");
  await model.activateWorkspace(a);
  const surface = model.state.surfaces.find(s => s.id === "file:a:a.txt")!, view = model.getEditor(surface, 1);
  view.call("paste", { text: "dirty " });
  let notifications = 0; const unsubscribe = model.subscribe(() => notifications++);
  model.selectSurface(surface.id, true); await model.openFile("a.txt");
  expect(notifications).toBe(0); expect(model.getEditor(surface, 1)).toBe(view); expect(view.frame.dirty).toBe(true);
  expect(view.text()).toBe("dirty one\ntwo\nthree"); unsubscribe();
});

test("closing a pending file does not resurrect its bytes, and initial surface publication is atomic", async () => {
  const { model, reads } = fixture({ gateReads: true }); await model.activateWorkspace(a);
  const observed: Array<{ active?: string; focused?: string }> = [];
  const unsubscribe = model.subscribe(() => observed.push({ active: model.state.activeSurfaceId, focused: model.getLayout().frame.focusedSurfaceId }));
  const open = model.openFile("a.txt"); await until(() => reads.length === 1);
  expect(observed).toHaveLength(1); expect(observed[0].active).toBe(observed[0].focused);
  model.closeSurface("file:a:a.txt"); reads.shift()!(); await open;
  expect(model.state.files["file:a:a.txt"]).toBeUndefined();
  expect(model.state.surfaces.map(surface => ({ id: surface.id, kind: surface.kind }))).toEqual([{ id: "welcome:a", kind: "welcome" }]);
  expect(model.state.activeSurfaceId).toBe("welcome:a"); unsubscribe();
});

test("idle recovery capture reuses snapshots but every native state mutation invalidates them", async () => {
  const { model } = fixture(); await model.activateWorkspace(a); await model.openFile("a.txt");
  const surface = model.state.surfaces[0], editor = model.getEditor(surface, 1);
  let calls = 0; const snapshot = editor.snapshot.bind(editor); editor.snapshot = () => { calls++; return snapshot(); };
  const first = model.captureSession(), idle = model.captureSession();
  expect(calls).toBe(1); expect(idle.editors["view:" + surface.id + "@1"]).toBe(first.editors["view:" + surface.id + "@1"]);
  editor.call("key", { key: "j", char: "j" }); const moved = model.captureSession(); expect(calls).toBe(2);
  expect(moved.editors["view:" + surface.id + "@1"]).not.toBe(first.editors["view:" + surface.id + "@1"]);
  editor.call("paste", { text: "unsaved " }); const edited = model.captureSession();
  expect(calls).toBe(3); expect((edited.editors["view:" + surface.id + "@1"] as any).text).toContain("unsaved ");
});
