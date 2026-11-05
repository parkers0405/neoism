// Real App/model/controller and native row geometry; no user session or daemon.
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { prepareBundledFonts } from "../../src/bundled-fonts";
import { loadConfig } from "../../src/config";
import { DaemonConnection } from "../../src/daemon/client";
import { WorkspaceModel } from "../../src/model";
import { App } from "../../src/ui/App";
import { AppVisualState } from "../../src/ui/AppVisualState";
import { createWindowKeyRouter } from "../../src/commands/window-key-router";
import { createSignal, onCleanup } from "solid-js";

const config = { ...loadConfig(["--config", "/nonexistent/database-sidebar-focus"], {}), scale: 1, reducedMotion: true, cursorBlink: false, terminalBlockUi: false };
const client = new DaemonConnection(config), model = new WorkspaceModel(client, config), visuals = new AppVisualState();
const workspace = { id: "sidebar-check", host_id: "test", title: "Sidebar test", root_dir: "/fixture" };
const connection = { id: "fruit", workspaceId: workspace.id, rootDir: workspace.root_dir, path: "fruit.db", title: "fruit.db" };
model.update({ activeWorkspace: workspace, workspaces: [workspace], loading: false });
model.setUI({ openWorkspaceIds: [workspace.id], databaseConnections: { [workspace.id]: [connection] }, workspaceSidebars: { [workspace.id]: "databases" } });
const schema = model.getDatabaseSchema(connection);
schema.state = { ...schema.state, objects: Array.from({ length: 60 }, (_, index) => ({ name: `table_${index}`, kind: "table" as const, sql: "", columns: [], indexes: [], triggers: [] })) };
model.newWelcome();
const renderer = createRenderer();
// Observe actual public native mutations, rather than inspecting source or
// replacing layout/focus. Forward every batch unchanged to the real renderer.
const styles = new Map<number, Record<string, unknown>>(), testIds = new Map<number, string>();
const applyBatch = renderer.applyBatch.bind(renderer);
renderer.applyBatch = (json: string) => {
  for (const [op, id, value, extra] of JSON.parse(json)) {
    if (op === "setStyle") styles.set(id, value);
    else if (op === "setCustomProp" && value === "testId") testIds.set(id, extra);
    else if (op === "destroyElement") { styles.delete(id); testIds.delete(id); }
  }
  return applyBatch(json);
};
prepareBundledFonts(renderer);
renderer.init({ title: "Neoism database sidebar focus verification", appId: "ai.neoism.gpuix.database-sidebar-check", width: 1200, height: 740 });
const loop = startFrameLoop(renderer);
const windowKeys = createWindowKeyRouter();
const root = render(() => {
  const [state, setState] = createSignal(model.state);
  const [schemaStyle, setSchemaStyle] = createSignal("");
  const styleTimer = setInterval(() => {
    const id = [...testIds].find(([, testId]) => testId === "database-schema")?.[0];
    if (id !== undefined) setSchemaStyle(JSON.stringify(styles.get(id)));
  }, 100);
  onCleanup(() => clearInterval(styleTimer));
  onCleanup(model.subscribe(() => setState(model.state)));
  return <div style={{ display: "flex", flexDirection: "column", width: "100%", height: "100%" }}>
    <App model={model} config={config} visuals={visuals} registerWindowKeys={windowKeys.register} />
    <text testId="database-sidebar-active-table" style={{ height: 18, minHeight: 18, fontSize: 10 }}>{state().surfaces.find(surface => surface.id === state().activeSurfaceId)?.databaseTable || "none"}</text>
    <text testId="database-sidebar-native-style" style={{ height: 1, minHeight: 1, fontSize: 1 }}>{schemaStyle()}</text>
  </div>;
}, { renderer, onKeyDown: windowKeys.dispatch });
const stop = () => { root.unmount(); loop.stop(); visuals.dispose(); model.dispose(); client.stop(); process.exit(0); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);