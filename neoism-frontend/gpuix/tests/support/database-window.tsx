// UI boundary only: no daemon, filesystem database, session recovery or user config.
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { createSignal } from "solid-js";
import { prepareBundledFonts } from "../../src/bundled-fonts";
import { loadConfig } from "../../src/config";
import { DatabaseView } from "../../src/database/DatabaseView";
import { DatabaseSidebar } from "../../src/database/DatabaseSidebar";
import type { DatabaseController } from "../../src/database/DatabaseController";
import { sameValue, type DatabaseObject, type DatabaseState, type DatabaseValue, type GridRow } from "../../src/database/types";

const width = Number(process.env.DATABASE_CHECK_WIDTH) || 1200;
const fixtureRows = Number(process.env.DATABASE_CHECK_ROWS) || 500;
const config = { ...loadConfig(["--config", "/nonexistent/native-database-fixture"], {}), scale: 1 };
const columns = ["id", "name", "origin", "quantity", "price"];
const objects: DatabaseObject[] = ["apples", "bananas", "fruit_audit"].map(name => ({
  name, kind: name === "fruit_audit" ? "view" : "table", sql: "",
  columns: columns.map((column, index) => ({ name: column, data_type: index === 0 || index === 3 ? "INTEGER" : index === 4 ? "REAL" : "TEXT", not_null: index === 0, primary_key: index === 0 ? 1 : 0, default_value: null })),
  indexes: [{ name: `idx_${name}_origin`, unique: false, columns: ["origin"] }], triggers: [],
}));
let report: (value: string) => void = () => {};
let sequence = 0;
const seed = (table: string): GridRow[] => Array.from({ length: fixtureRows }, (_, index) => {
  const values: DatabaseValue[] = [{ kind: "integer", value: String(index + 1) }, { kind: "text", value: `${table === "bananas" ? "Banana" : "Apple"} ${index + 1}` },
    { kind: "text", value: ["Italy", "France", "USA", "Japan", "Spain"][index % 5]! }, { kind: "integer", value: String(10 + index) }, { kind: "real", value: 1.25 + index / 100 }];
  return { key: `row:${index}`, values, identity: { id: values[0]! }, original: Object.fromEntries(columns.map((column, i) => [column, values[i]!])), kind: "existing" };
});
function createTable(table: string) {
const listeners = new Set<() => void>();
const model = {
  path: "/fixture/fruit.db",
  state: { objects, tabs: [], table, columns, rows: seed(table), total: fixtureRows, offset: 0, limit: fixtureRows,
    editable: table !== "fruit_audit", loading: false, applying: false, filters: {}, pending: 0 } as DatabaseState,
  subscribe(listener: () => void) { listeners.add(listener); return () => listeners.delete(listener); },
  async initialize() {},
  async selectTable() { throw new Error("UI must open a separate table buffer, not navigate its controller"); },
  closeTable(name: string) { update({ tabs: model.state.tabs.filter(tab => tab !== name) }); },
  async refresh() {}, async page() {}, async sortColumn(column: string) { update({ sort: { column, descending: !model.state.sort?.descending } }); },
  async loadValues(column: string) {
    const unique: { value: DatabaseValue; count: number }[] = [];
    for (const row of seed(model.state.table || "apples")) {
      const value = row.values[columns.indexOf(column)]!;
      const found = unique.find(item => sameValue(item.value, value)); if (found) found.count++; else unique.push({ value, count: 1 });
    }
    return { values: unique, truncated: false };
  },
  async setFilter(column: string, values: DatabaseValue[] | undefined) {
    const filters = { ...model.state.filters }; if (values === undefined) delete filters[column]; else filters[column] = values;
    const rows = seed(model.state.table || "apples").filter(row => Object.entries(filters).every(([name, allowed]) => allowed.some(value => sameValue(value, row.values[columns.indexOf(name)]!))));
    update({ filters, rows, total: rows.length });
  },
  selectCell(row: string, column: number) { update({ selected: { row, column } }); },
  editCell(key: string, column: number, value: DatabaseValue | undefined) {
    const rows = model.state.rows.map(row => { if (row.key !== key) return row; const values = [...row.values]; if (value === undefined) delete values[column]; else values[column] = value; return { ...row, values, kind: row.kind === "insert" ? "insert" as const : "update" as const }; });
    update({ rows, pending: rows.filter(row => row.kind !== "existing").length });
  },
  addRow() { const key = `new:${++sequence}`; update({ rows: [...model.state.rows, { key, values: [], original: {}, identity: null, kind: "insert" }], pending: model.state.pending + 1, selected: { row: key, column: 0 } }); },
  deleteRow(key: string) { update({ rows: model.state.rows.map(row => row.key === key ? { ...row, kind: "delete" } : row), pending: model.state.pending + 1 }); },
  async apply() { update({ pending: 0, notice: "Fixture applied (memory only)" }); },
  discard() { update({ rows: seed(model.state.table || "apples"), pending: 0 }); },
  clearError() { update({ error: undefined }); },
};
function update(patch: Partial<DatabaseState>) {
  model.state = { ...model.state, ...patch }; for (const listener of listeners) listener();
  publish(model.state);
}
return model;
}
function publish(state: DatabaseState) { report(`table=${state.table}; selected=${state.selected?.row}:${state.selected?.column}; filters=${Object.keys(state.filters).join(",")}; pending=${state.pending}`); }
const tables = new Map(objects.map(object => [object.name, createTable(object.name)]));
const schema = { ...createTable("apples"), state: { ...tables.get("apples")!.state, rows: [], columns: [], table: undefined, editable: false } };
const renderer = createRenderer();
prepareBundledFonts(renderer);
renderer.init({ title: `Neoism database verification ${width}`, appId: "ai.neoism.gpuix.database-check", width, height: 740 });
const loop = startFrameLoop(renderer);
const root = render(() => {
  const [status, setStatus] = createSignal("ready"); report = setStatus;
  const [controller, setController] = createSignal(tables.get("apples")!);
  const [zone, setZone] = createSignal("content");
  // Keep the requested pane width explicit even when a tiling WM initially
  // ignores the window dimensions; the launcher sizes only this fixture PID.
  return <div testId="database-fixture" style={{ width, maxWidth: "100%", height: "100%", display: "flex", flexDirection: "column", backgroundColor: config.theme.bg }}>
    <div testId="database-fixture-body" style={{ flexGrow: 1, flexBasis: 0, minHeight: 0, display: "flex", flexDirection: "row" }}>
      <div style={{ width: 210, flexShrink: 0, minHeight: 0 }}>
        <DatabaseSidebar controller={schema as unknown as DatabaseController} config={config} connections={[{ id: "fruit", title: "fruit.db", path: "/fixture/fruit.db" }]} activeId="fruit"
          active={zone() === "sidebar"} onFocus={() => setZone("sidebar")} onConnection={() => setZone("sidebar")}
          onTable={name => { const next = tables.get(name)!; setController(next); setZone("content"); publish(next.state); }} onBrowse={() => report("browse requested")} />
      </div>
      <div style={{ flexGrow: 1, flexBasis: 0, minWidth: 0, minHeight: 0 }}>
        <DatabaseView controller={controller() as unknown as DatabaseController} config={config} active={zone() === "content"} onFocus={() => setZone("content")} />
      </div>
    </div>
    <text testId="database-fixture-status" style={{ height: 20, minHeight: 20, fontSize: 10, color: config.theme.muted }}>{status()}</text>
  </div>;
}, { renderer });
const stop = () => { root.unmount(); loop.stop(); process.exit(0); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);