import * as Effect from "effect/Effect";
import type { DaemonConnection } from "../daemon/client";
import { expectVariant, isRecord } from "../daemon/protocol";
import type { DatabaseRequest, DatabaseObject, DatabaseValue } from "./protocol";
import { sameValue, type DatabaseState, type GridRow } from "./types";

type Values = { values: { value: DatabaseValue; count: number }[]; truncated: boolean };
type Rows = { columns: string[]; rows: DatabaseValue[][]; identities: (Record<string, DatabaseValue> | null)[]; total: number; editable: boolean };

/** A database tab owns its draft independently of view mounts and theme changes. */
export class DatabaseController {
  state: DatabaseState = {
    objects: [], tabs: [], columns: [], rows: [], total: 0, offset: 0, limit: 50,
    editable: false, loading: false, applying: false, filters: {}, pending: 0,
  };
  private listeners = new Set<() => void>();
  private loaded = false;
  private disposed = false;
  private generation = 0;
  private snapshot?: Rows;
  private uncertain = false;
  private initializing?: Promise<void>;

  constructor(readonly client: DaemonConnection, readonly root: string, readonly path: string,
    private readonly onChange: () => void = () => {}, private readonly initialTable?: string | null) {}

  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(patch: Partial<DatabaseState>) {
    if (this.disposed) return;
    this.state = { ...this.state, ...patch };
    this.onChange();
    for (const listener of this.listeners) listener();
  }
  clearError() { this.update({ error: undefined, notice: undefined }); }
  private fail(error: unknown) { this.update({ error: error instanceof Error ? error.message : String(error) }); }
  private request(message: DatabaseRequest) {
    return Effect.runPromise(this.client.request("Database", message, this.root));
  }
  private canNavigate() {
    if (this.state.loading || this.state.applying) return false;
    if (this.state.pending) { this.fail(new Error("Apply or discard the pending changes first.")); return false; }
    return !this.disposed;
  }
  private filters(exclude?: string) {
    return Object.entries(this.state.filters).filter(([column]) => column !== exclude).map(([column, values]) => ({ column, values }));
  }

  async initialize() {
    if (this.initializing) return this.initializing;
    if (this.loaded || this.state.loading || this.disposed) return;
    this.initializing = this.refresh();
    try { await this.initializing; } finally { this.initializing = undefined; }
  }
  highlightTable(name: string) { if (this.initialTable === null && name !== this.state.table) this.update({ table: name }); }
  async refresh() {
    if (!this.canNavigate()) return;
    const generation = ++this.generation;
    this.update({ loading: true, error: undefined });
    try {
      const reply = expectVariant(await this.request({ Schema: { path: this.path } }), "Schema");
      if (!isRecord(reply) || !Array.isArray(reply.objects)) throw new Error("Invalid database schema reply");
      if (this.disposed || generation !== this.generation) return;
      const objects = reply.objects as DatabaseObject[];
      if (typeof this.initialTable === "string" && !objects.some(object => object.name === this.initialTable)) throw new Error(`Table or view no longer exists: ${this.initialTable}`);
      const table = this.initialTable === null ? (objects.some(object => object.name === this.state.table) ? this.state.table : undefined)
        : this.initialTable ?? (objects.some(object => object.name === this.state.table) ? this.state.table : objects[0]?.name);
      this.loaded = true;
      this.uncertain = false;
      this.update({ objects, table, tabs: [...new Set([...this.state.tabs.filter(name => objects.some(object => object.name === name)), ...(table ? [table] : [])])] });
      if (table && this.initialTable !== null) await this.browse(generation);
      else this.update({ columns: [], rows: [], total: 0, editable: false });
    } catch (error) { if (generation === this.generation) { this.update({ editable: false }); this.fail(error); } }
    finally { if (generation === this.generation) this.update({ loading: false }); }
  }
  private async browse(generation: number) {
    if (!this.state.table) return;
    const reply = expectVariant(await this.request({ Browse: {
      path: this.path, table: this.state.table, limit: this.state.limit, offset: this.state.offset,
      sort: this.state.sort, filters: this.filters(),
    } }), "Rows");
    if (this.disposed || generation !== this.generation) return;
    if (!isRecord(reply) || !Array.isArray(reply.columns) || !Array.isArray(reply.rows) || !Array.isArray(reply.identities)) throw new Error("Invalid database rows reply");
    const snapshot = reply as unknown as Rows;
    if (snapshot.rows.length !== snapshot.identities.length || snapshot.rows.some(row => row.length !== snapshot.columns.length)) throw new Error("Invalid database row shape");
    if (this.state.offset > 0 && this.state.offset >= snapshot.total) {
      this.update({ offset: Math.max(0, Math.floor((snapshot.total - 1) / this.state.limit) * this.state.limit) });
      await this.browse(generation);
      return;
    }
    this.snapshot = snapshot;
    this.update({ columns: snapshot.columns, rows: this.gridRows(snapshot), total: snapshot.total, editable: snapshot.editable, pending: 0, selected: undefined });
  }
  private gridRows(snapshot: Rows): GridRow[] {
    return snapshot.rows.map((values, index) => ({
      key: `row:${this.state.offset + index}`, values: [...values], identity: snapshot.identities[index] ?? null,
      original: Object.fromEntries(snapshot.columns.map((column, i) => [column, values[i]!])), kind: "existing",
    }));
  }
  private async navigate(patch: Partial<DatabaseState>) {
    if (!this.canNavigate()) return;
    const generation = ++this.generation;
    this.snapshot = undefined;
    // Do not leave the previous table's rows editable under a new table name.
    const switching = patch.table !== undefined && patch.table !== this.state.table;
    this.update({ ...patch, ...(switching ? { columns: [], rows: [], total: 0 } : {}), editable: false, selected: undefined, loading: true, error: undefined, notice: undefined });
    try { await this.browse(generation); }
    catch (error) { if (generation === this.generation) this.fail(error); }
    finally { if (generation === this.generation) this.update({ loading: false }); }
  }
  async selectTable(name: string) {
    if (name === this.state.table || !this.state.objects.some(object => object.name === name)) return;
    await this.navigate({ table: name, tabs: [...new Set([...this.state.tabs, name])], offset: 0, sort: undefined, filters: {} });
  }
  closeTable(name: string) {
    if (!this.canNavigate()) return;
    const tabs = this.state.tabs.filter(tab => tab !== name);
    if (this.state.table !== name) { this.update({ tabs }); return; }
    if (tabs.length) void this.navigate({ table: tabs[tabs.length - 1], tabs, offset: 0, sort: undefined, filters: {} });
    else { this.snapshot = undefined; this.update({ tabs, table: undefined, columns: [], rows: [], total: 0, editable: false, selected: undefined }); }
  }
  async page(delta: number) {
    const offset = Math.max(0, this.state.offset + delta * this.state.limit);
    if (offset === this.state.offset || offset >= this.state.total) return;
    await this.navigate({ offset });
  }
  async setPageSize(limit: number) {
    if (![25, 50, 100, 200].includes(limit) || limit === this.state.limit) return;
    await this.navigate({ limit, offset: 0 });
  }
  async sortColumn(column: string) {
    if (!this.state.columns.includes(column)) return;
    const before = this.state.sort;
    await this.navigate({ offset: 0, sort: before?.column === column && before.descending ? undefined : { column, descending: before?.column === column } });
  }
  async setFilter(column: string, values: DatabaseValue[] | undefined) {
    if (!this.state.columns.includes(column)) return;
    const filters = Object.fromEntries([
      ...Object.entries(this.state.filters).filter(([name]) => name !== column),
      ...(values === undefined ? [] : [[column, values] as const]),
    ]);
    await this.navigate({ filters, offset: 0 });
  }
  async loadValues(column: string): Promise<Values> {
    if (!this.state.table || !this.state.columns.includes(column)) return { values: [], truncated: false };
    return expectVariant(await this.request({ Values: { path: this.path, table: this.state.table, column, filters: this.filters(column) } }), "Values") as Values;
  }
  selectCell(row: string, column: number) {
    if (this.state.selected?.row === row && this.state.selected.column === column) return;
    if (column >= 0 && column < this.state.columns.length && this.state.rows.some(value => value.key === row)) this.update({ selected: { row, column } });
  }
  private canEdit() {
    if (this.uncertain) { this.fail(new Error("The previous Apply could not be confirmed. Discard the draft and refresh to inspect the database before editing again.")); return false; }
    return this.state.editable && !this.state.loading && !this.state.applying && !this.disposed;
  }
  private setRows(rows: GridRow[]) {
    this.update({ rows, pending: rows.filter(row => row.kind !== "existing").length, error: undefined, notice: undefined });
  }
  editCell(key: string, column: number, value: DatabaseValue | undefined) {
    if (!this.canEdit()) return;
    const name = this.state.columns[column];
    const target = this.state.rows.find(row => row.key === key);
    if (!name || !target || target.kind === "delete" || target.kind !== "insert" && !target.identity) return;
    if (target.kind !== "insert" && (value === undefined || target.values[column]?.kind === "blob")) return;
    this.setRows(this.state.rows.map(row => {
      if (row !== target) return row;
      const values = [...row.values];
      // Sparse insert values distinguish an omitted/default column from NULL.
      if (value === undefined) delete values[column]; else values[column] = value;
      const kind = row.kind === "insert" ? "insert" : this.state.columns.every((name, i) => sameValue(values[i]!, row.original[name]!)) ? "existing" : "update";
      return { ...row, values, kind };
    }));
  }
  addRow() {
    if (!this.canEdit() || !this.state.table) return;
    const key = `new:${crypto.randomUUID()}`;
    this.setRows([...this.state.rows, { key, values: [], identity: null, original: {}, kind: "insert" }]);
    this.selectCell(key, 0);
  }
  deleteRow(key: string) {
    if (!this.canEdit()) return;
    const row = this.state.rows.find(row => row.key === key);
    if (!row || row.kind !== "insert" && !row.identity) return;
    this.setRows(row.kind === "insert" ? this.state.rows.filter(row => row.key !== key)
      : this.state.rows.map(row => row.key === key ? { ...row, kind: "delete" } : row));
  }
  discard() {
    if (this.state.applying || this.state.loading || this.disposed) return;
    const uncertain = this.uncertain;
    this.uncertain = false;
    this.update({ rows: this.snapshot ? this.gridRows(this.snapshot) : [], pending: 0, selected: undefined, error: undefined, notice: undefined });
    if (uncertain) void this.refresh();
  }
  async apply() {
    if (!this.canEdit() || !this.state.pending || !this.state.table) return;
    const changes = this.state.rows.filter(row => row.kind !== "existing").map(row => ({
      kind: row.kind as "insert" | "update" | "delete",
      values: Object.fromEntries(this.state.columns.flatMap((column, i) => {
        const value = row.values[i];
        return value !== undefined && row.kind !== "delete" && (row.kind === "insert" || !sameValue(value, row.original[column]!)) ? [[column, value]] : [];
      })),
      ...(row.identity ? { identity: row.identity } : {}),
      ...(row.kind !== "insert" ? { original: row.original } : {}),
    }));
    this.update({ applying: true, error: undefined });
    try {
      const reply = expectVariant(await this.request({ Apply: { path: this.path, table: this.state.table, changes } }), "Applied") as { affected: number };
      if (this.disposed) return;
      this.snapshot = undefined;
      this.update({ pending: 0, rows: [], applying: false, notice: `${reply.affected} row${reply.affected === 1 ? "" : "s"} saved` });
      await this.refresh();
    } catch (error) {
      if (/timed out|offline|disconnect/i.test(error instanceof Error ? error.message : String(error))) {
        this.uncertain = true;
        this.fail(new Error("Apply was not confirmed. It may have committed. Discard the draft and refresh to inspect the database; do not retry blindly."));
      } else this.fail(error);
    } finally { this.update({ applying: false }); }
  }
  dispose() { this.disposed = true; this.generation++; this.listeners.clear(); }
}