import { expect, test } from "bun:test";
import * as Effect from "effect/Effect";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
import { decodeSession } from "../src/session-store";
import { workspaceSidebar } from "../src/ui/workspace-sidebar";
import { createAgentDaemon } from "./support/agent-daemon";

test("SQLite files open a recoverable database tab instead of reading binary bytes into the code editor", async () => {
  const config = loadConfig(["--config", "/nonexistent/database-workspace"], {});
  const model = new WorkspaceModel(new DaemonConnection(config), config);
  const workspace = { id: "local", host_id: "local", title: "App", root_dir: "/app" };
  model.update({ workspaces: [workspace], activeWorkspace: workspace });
  try {
    await model.openFile("data/fruit.sqlite3");
    const connection = {...model.activeDatabaseConnection()!,id:"saved-connection"};
    model.setUI({databaseConnections:{[workspace.id]:[connection]},databaseSelections:{[workspace.id]:connection.id}});
    expect(connection).toMatchObject({ path: "data/fruit.sqlite3", rootDir: "/app", title: "fruit.sqlite3" });
    expect(workspaceSidebar(model.state.ui, workspace.id)).toBe("databases");
    const schema = model.getDatabaseSchema(connection);
    schema.state = { ...schema.state, objects: ["fruit", "other"].map(name => ({ name, kind: "table", sql: "", columns: [], indexes: [], triggers: [] })) };
    model.openDatabaseTable(connection, "fruit");
    const surface = model.state.surfaces.find(surface => surface.id === model.state.activeSurfaceId)!;
    expect(surface).toMatchObject({ kind: "page", page: "database", databaseTable: "fruit", path: "data/fruit.sqlite3", rootDir: "/app", title: "fruit" });
    expect(model.getLayout().frame.leaves.find(tab => tab.surfaceId === surface.id)?.page).toBe("database");
    expect(model.state.files[surface.id]).toBeUndefined();
    expect(model.editors.size).toBe(0);
    model.openDatabaseTable(connection, "fruit");
    expect(model.state.surfaces.filter(surface => surface.page === "database")).toHaveLength(1);
    const db = model.getDatabase(surface);
    expect(model.getDatabase(surface)).toBe(db);
    expect(decodeSession(JSON.parse(JSON.stringify(model.captureSession())))?.surfaces).toContainEqual(surface);

    db.state = { ...db.state, editable: true, table: "fruit", columns: ["id"] };
    db.addRow();
    expect(model.isSurfaceModified(surface.id)).toBe(true);
    model.openDatabaseTable(connection, "other");
    const other = model.state.surfaces.find(entry => entry.databaseTable === "other")!;
    expect(other).toBeDefined();
    expect(model.getLayout().frame.leaves.some(leaf => leaf.surfaceId === surface.id)).toBe(true);
    expect(model.getLayout().frame.leaves.some(leaf => leaf.surfaceId === other.id)).toBe(true);
    expect(model.getDatabase(other)).not.toBe(db);
    expect(db.state.pending).toBe(1);
    model.selectSurface(surface.id);
    expect(model.getDatabaseSchema(connection).state.table).toBe("fruit");
    model.closeSurface(surface.id);
    expect(model.state.surfaces).toContainEqual(surface);
    expect(model.state.error).toContain("Apply or discard");
    db.discard();
    expect(model.isSurfaceModified(surface.id)).toBe(false);
    model.closeSurface(surface.id);
    expect(model.state.surfaces.some(entry => entry.id === surface.id)).toBe(false);
    expect(model.databaseConnections()).toHaveLength(1);
    const restored = decodeSession(JSON.parse(JSON.stringify(model.captureSession())))!;
    expect(workspaceSidebar(restored.ui!, workspace.id)).toBe("databases");
    expect(model.databaseConnections("another-workspace")).toEqual([]);
  } finally { model.dispose(); }
});

test("legacy layout routes recover page metadata from persisted surfaces", () => {
  const config = loadConfig(["--config", "/nonexistent/database-layout-recovery"], {});
  const first = new WorkspaceModel(new DaemonConnection(config), config);
  const recovered = new WorkspaceModel(new DaemonConnection(config), config);
  const workspace = { id: "local", host_id: "local", title: "App", root_dir: "/app" };
  const surface = { id: "opaque", kind: "page", page: "database", title: "Extensions", workspaceId: workspace.id } as const;
  try {
    first.update({ workspaces: [workspace], activeWorkspace: workspace, surfaces: [surface] });
    const snapshot = first.getLayout().snapshot();
    for (const route of Object.values(snapshot.routes)) delete route.page;
    recovered.update({ workspaces: [workspace], activeWorkspace: workspace, surfaces: [surface], ui: { layouts: { local: snapshot } } });
    expect(recovered.getLayout().frame.panes[0].tabs[0].page).toBe("database");
    expect(Object.values(recovered.getLayout().snapshot().routes)[0].page).toBe("database");
  } finally { first.dispose(); recovered.dispose(); }
});

test("host workspace teardown waits for database transactions and releases database sidebar state", async () => {
  const daemon = createAgentDaemon();
  const config = { ...loadConfig(["--config", "/nonexistent/database-close"], {}), endpoint: daemon.endpoint, authToken: undefined };
  const client = new DaemonConnection(config);
  const request = client.request.bind(client);
  let finishApply: ((reply: Effect.Effect<unknown>) => void) | undefined;
  client.request = ((family, message, root) => {
    if (family !== "Database") return request(family, message, root);
    if (typeof message === "object" && message && "Apply" in message) return Effect.callback<unknown>(resume => { finishApply = resume; });
    if (typeof message === "object" && message && "Schema" in message) return Effect.succeed({ Schema: { objects: [{ name: "fruit", kind: "table", sql: "", columns: [], indexes: [], triggers: [] }] } });
    return Effect.succeed({ Rows: { columns: ["id"], rows: [], identities: [], total: 0, editable: true } });
  }) as typeof client.request;
  const model = new WorkspaceModel(client, config);
  try {
    model.start(); client.start();
    const deadline = Date.now() + 2500;
    while (!model.state.activeWorkspace || model.state.loading) { if (Date.now() > deadline) throw new Error("Database workspace did not connect"); await Bun.sleep(5); }
    await model.openFile("fruit.db");
    const connection = model.activeDatabaseConnection()!;
    const surface = model.state.surfaces.find(entry => entry.id === model.state.activeSurfaceId)!;
    const database = model.getDatabase(surface);
    await database.initialize();
    database.addRow();
    expect(await model.closeWorkspace(false, connection.workspaceId)).toBe(false);
    const apply = database.apply();
    const applyDeadline = Date.now() + 2500;
    while (!finishApply) { if (Date.now() > applyDeadline) throw new Error("Apply did not start"); await Bun.sleep(1); }
    expect(database.state.applying).toBe(true);
    expect(await model.closeWorkspace(true, connection.workspaceId)).toBe(false);
    expect(model.state.error).toContain("transaction to finish");
    expect(daemon.workspaceRequests.some(entry => entry.tag === "CloseWindowWorkspace")).toBe(false);
    finishApply(Effect.succeed({ Applied: { affected: 1 } }));
    await apply;
    expect(database.state.pending).toBe(0);
    expect(await model.closeWorkspace(false, connection.workspaceId)).toBe(true);
    expect(model.databaseConnections(connection.workspaceId)).toEqual([]);
    expect((model.state.ui.databaseSelections as Record<string, unknown>)[connection.workspaceId]).toBeUndefined();
    expect((model.state.ui.workspaceSidebars as Record<string, unknown>)[connection.workspaceId]).toBeUndefined();
    model.openDatabaseTable(connection, "fruit");
    expect(model.state.surfaces.some(entry => entry.workspaceId === connection.workspaceId)).toBe(false);
    expect(daemon.workspaceRequests.filter(entry => entry.tag === "CloseWindowWorkspace")).toHaveLength(1);
  } finally { model.dispose(); client.stop(); daemon.stop(); }
});