import assert from "node:assert/strict";
import { createRoot } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { DatabaseView } from "../../src/database/DatabaseView";
import { DatabaseSidebar } from "../../src/database/DatabaseSidebar";
import type { DatabaseController } from "../../src/database/DatabaseController";
import type { DatabaseState, DatabaseValue } from "../../src/database/types";
import { loadConfig } from "../../src/config";
import { createRecordingAppRenderer, type RecordedAppNode } from "./full-app-recording-renderer";

const config = loadConfig(["--config", "/nonexistent/database-view"], {});
const listeners = new Set<() => void>();
const edits: (DatabaseValue | undefined)[] = [];
let applies = 0, unsubscribed = false, interceptedSaves = 0, interceptedNavigation = 0;
let browses = 0, sidebarShortcuts = 0, registered = false;
const opened: string[] = [], connections: string[] = [];
const model = {
  path: "/fixture/fruit.db",
  state: {
    objects: [{ name: "grapes", kind: "table", sql: "", columns: [{ name: "id", data_type: "INTEGER", not_null: true, primary_key: 1, default_value: null }], indexes: [], triggers: [] }],
    tabs: ["grapes"], table: "grapes", columns: Array.from({ length: 20 }, (_, i) => `col${i}`),
    rows: Array.from({ length: 500 }, (_, i) => ({ key: `row:${i}`, values: Array.from({ length: 20 }, (_, c) => ({ kind: "text" as const, value: `${i}:${c}` })), identity: i === 1 ? null : { id: { kind: "integer" as const, value: String(i) } }, original: {}, kind: "existing" as const })),
    total: 500, offset: 0, limit: 500, editable: true, loading: false, applying: false, filters: {}, pending: 0,
  } as DatabaseState,
  subscribe(listener: () => void) { listeners.add(listener); return () => { unsubscribed = true; listeners.delete(listener); }; },
  selectCell(row: string, column: number) { update({ selected: { row, column } }); },
  editCell(_row: string, _column: number, value: DatabaseValue | undefined) { edits.push(value); },
  deleteRow(key: string) { update({ rows: model.state.rows.map(row => row.key === key ? { ...row, kind: "delete" } : row), pending: 1 }); },
  async apply() { applies++; },
  async loadValues() { return { values: [{ value: { kind: "text" as const, value: "Italy" }, count: 3 }, { value: { kind: "null" as const }, count: 2 }], truncated: false }; },
  async setFilter(column: string, values: DatabaseValue[] | undefined) { const filters = { ...model.state.filters }; if (values === undefined) delete filters[column]; else filters[column] = values; update({ filters }); },
  clearError() {},
};
function update(patch: Partial<DatabaseState>) { model.state = { ...model.state, ...patch }; for (const listener of listeners) listener(); }
const host = createRecordingAppRenderer();
const frames:unknown[][][]=[];
const applyBatch=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=json=>{frames.push(JSON.parse(json));return applyBatch(json);};
const root = createRoot(host.renderer);
const scrollCalls:{id:number;x:number;y:number}[]=[];
host.renderer.scrollTo=(id,x,y)=>{scrollCalls.push({id,x,y});};
const pump = () => host.pump(root);
const panTo=async(x:number,y:number)=>{
  const plane=find("database-data-plane");
  root.dispatch({elementId:find("database-grid").id,eventType:"scroll",precise:true,deltaX:-Number(plane.style.left||0)-x,deltaY:-Number(plane.style.top||0)-y});
  await pump();
};
const find = (id: string) => { const node = host.find(id); assert.ok(node, `Missing ${id}`); return node; };
const label = (name: string) => { const node = [...host.nodes.values()].find(node => node.props["aria-label"] === name && node.props.role === "button"); assert.ok(node, `Missing action ${name}`); return node; };
const click = async (node: RecordedAppNode) => { root.dispatch({ elementId: node.id, eventType: "click", button: 0 }); await pump(); };
const key = async (value: string, modifiers: { ctrl?: boolean } = {}) => { root.dispatch({ elementId: find("database-workbench").id, eventType: "keyDown", key: value, modifiers: { ctrl: false, shift: false, alt: false, cmd: false, ...modifiers } }); await pump(); };
try {
  root.render(() => {
    const [activeId, setActiveId] = createSignal("fruit");
    const [gridActive, setGridActive] = createSignal(false);
    return <div><DatabaseSidebar controller={model as unknown as DatabaseController} config={config}
      connections={[{ id: "fruit", title: "fruit.db", path: "/fixture/fruit.db" }, { id: "other", title: "other.db", path: "/fixture/other.db" }]} activeId={activeId()}
      onConnection={id => { connections.push(id); setActiveId(id); }} onTable={name => opened.push(name)} onBrowse={() => browses++}
      registerHost={host => registered = !!host} onCustomKey={event => { if (event.modifiers?.alt) { sidebarShortcuts++; return true; } if (["up", "down", "left", "right", "enter"].includes(event.key || "")) { interceptedNavigation++; return true; } return false; }} />
    <DatabaseView controller={model as unknown as DatabaseController} config={config} active={gridActive()} onFocus={() => setGridActive(true)} onCustomKey={event => {
    if (event.modifiers?.ctrl && event.key === "s") { interceptedSaves++; return true; }
    if (["up", "down", "left", "right", "enter", "arrowright", "arrowdown"].includes(event.key?.toLowerCase() || "")) { interceptedNavigation++; return true; }
    return false;
  }} /></div>;
  });
  await pump();
  assert.ok(registered, "Sidebar registers its native focus host");
  assert.equal(host.find("database-tabs"), undefined, "Table views do not own a nested tab strip");
  const descendants = (node: RecordedAppNode): RecordedAppNode[] => [node, ...node.children.flatMap(id => { const child = host.nodes.get(id); return child ? descendants(child) : []; })];
  assert.ok(!descendants(find("database-workbench")).some(node => node.props.testId === "database-sidebar" || node.props.testId === "database-schema"), "Schema is outside the normal table buffer");
  await click(find("database-browse")); assert.equal(browses, 1);
  await click(find("database-connection:other")); assert.deepEqual(connections, ["other"]);
  await click(find("database-connection:fruit"));
  const sidebarKey = async (key: string, alt = false) => { root.dispatch({ elementId: find("database-sidebar").id, eventType: "keyDown", key, modifiers: { ctrl: false, cmd: false, shift: false, alt } }); await pump(); };
  await sidebarKey("down"); await sidebarKey("right"); await sidebarKey("enter");
  assert.deepEqual(opened, ["grapes"], "Keyboard table activation delegates to normal buffer callback");
  await sidebarKey("right"); find("database-section:grapes:Columns");
  assert.ok([...host.nodes.values()].some(node => node.props["aria-label"] === "id: INTEGER [PK] [NN]"));
  await sidebarKey("left"); assert.equal(host.find("database-section:grapes:Columns"), undefined);
  await sidebarKey("right", true); assert.equal(sidebarShortcuts, 1, "Alt navigation is delegated before tree navigation");
  await click(find("database-table:grapes")); assert.deepEqual(opened, ["grapes", "grapes"]); assert.equal(model.state.table, "grapes");
  assert.ok([...host.nodes.values()].filter(node => node.props.role === "row").length < 80, "Rows mount within the viewport plus bounded scroll buffer");
  assert.ok([...host.nodes.values()].filter(node => node.props.role === "gridcell").length < 800, "Only buffered viewport rows and viewport columns mount");
  const retained = find("database-cell:row:10:3").id, retainedHeader = find("database-filter:col3").id;
  root.dispatch({elementId:find("database-grid").id,eventType:"scroll",precise:false,deltaY:-1});
  await pump();
  assert.equal(find("database-data-plane").style.top,-75*config.scale,"Mouse wheel scrolls three table rows instead of one pixel");
  root.dispatch({elementId:find("database-header").id,eventType:"scroll",precise:false,deltaY:-1,modifiers:{shift:true,ctrl:false,alt:false,cmd:false}});
  await pump();
  assert.equal(find("database-data-plane").style.left,-75*config.scale,"Shift-wheel uses the same line conversion horizontally");
  assert.equal(find("database-data-plane").style.top,-75*config.scale,"Shift-wheel leaves the vertical offset unchanged");
  await panTo(0,0);
  root.dispatch({elementId:find("database-grid").id,eventType:"scroll",precise:true,deltaY:-2.5});
  await pump();
  assert.equal(find("database-data-plane").style.top,-2.5,"Trackpad pixel deltas remain unscaled");
  await panTo(0,0);
  for (const [x, y] of [[0, 1], [0, 200], [450, 200]]) {
    await panTo(x!*config.scale,y!*config.scale);
    assert.equal(find("database-cell:row:10:3").id, retained, "Overlapping cells must remain mounted during horizontal/vertical scroll");
    assert.equal(find("database-filter:col3").id, retainedHeader, "Scrolling preserves the shared column header");
  }
  await panTo(1500*config.scale,2500*config.scale);
  find("database-cell:row:100:10"); assert.equal(host.find("database-cell:row:0:0"), undefined);
  for (const y of [10000, 0, 9000, 0]) {
    await panTo(0,y*config.scale);
    const top = Math.floor(y / 25);
    find(`database-cell:row:${top}:0`); find(`database-cell:row:${top + 24}:0`);
    if (top > 0) find(`database-cell:row:${top - 8}:0`);
    find(`database-cell:row:${top + 32}:0`);
    assert.ok([...host.nodes.values()].filter(node => node.props.role === "gridcell").length < 800, "Fast reversals keep viewport edges buffered without mounting all data");
  }
  await panTo(0,0);
  const startFrame=frames.length;
  await panTo(25*config.scale,50*config.scale);
  const planeIds=["database-header-plane","database-data-plane","database-gutter-plane"].map(id=>find(id).id);
  assert.ok(frames.slice(startFrame).some(frame=>planeIds.every(id=>frame.some(op=>op[0]==="setStyle"&&op[1]===id))),"Header, cells and row numbers translate in the same native mutation frame");
  assert.equal(find("database-header-plane").style.left,find("database-data-plane").style.left);
  assert.equal(find("database-gutter-plane").style.top,find("database-data-plane").style.top);
  assert.equal(find("database-gutter-plane").style.left,0);
  await panTo(0,200*config.scale);
  const horizontalThumb=find("database-scrollbar-thumb-horizontal");
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseDown",x:100,button:0});
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseMove",x:10000,pressedButton:0});await pump();
  const dragged=Number(find("database-data-plane").style.left);assert.ok(dragged<0,"Dragging pans the data plane");
  assert.equal(find("database-data-plane").style.top,-200*config.scale,"Horizontal dragging preserves the vertical position");
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseMove",x:100000,pressedButton:0});await pump();
  assert.equal(find("database-data-plane").style.left,dragged,"Dragging beyond the end clamps at the same offset");
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseUp",x:10000,button:0});
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseMove",x:-10000,pressedButton:0});await pump();
  assert.equal(find("database-data-plane").style.left,dragged,"Releasing ends capture");
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseDown",x:10000,button:0});
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseMove",x:-10000,pressedButton:0});
  root.dispatch({elementId:horizontalThumb.id,eventType:"mouseUp",x:-10000,button:0});await pump();
  assert.equal(Math.abs(Number(find("database-data-plane").style.left)),0);
  assert.equal(scrollCalls.length,0,"There is no independently scrolling native handle to outrun the frozen panes");
  await panTo(0,0);
  await click(find("database-cell:row:1:0")); await key("enter"); assert.equal(host.find("database-cell-editor"), undefined, "Identity-less rows are not editable");
  assert.equal(host.focused, find("database-workbench").id, "Clicking a cell restores native grid focus from parked chrome");
  update({ applying: true }); await click(find("database-cell:row:0:0")); await key("enter"); assert.equal(host.find("database-cell-editor"), undefined); update({ applying: false });
  await key("ArrowRight"); assert.equal(model.state.selected?.column, 1, "Arrow keys move the selected cell before chrome navigation");
  await key("ArrowDown"); assert.equal(model.state.selected?.row, "row:1");
  assert.equal(interceptedNavigation, 0, "Plain grid/tree navigation never leaks to app chrome handlers");
  update({rows:model.state.rows.slice(0,100),limit:100,total:100});await pump();
  for(const y of [0,1800,0,1800]){
    await panTo(0,y*config.scale);
    const top=Math.floor(y/25);
    find(`database-cell:row:${top}:0`);find(`database-cell:row:${Math.min(99,top+20)}:0`);
    assert.equal(find("database-gutter-plane").style.top,find("database-data-plane").style.top,"Paged viewport edges and frozen numbers commit together after a fast reversal");
  }
  update({ rows: [{ key: "blob", values: [{ kind: "blob", value: "AAAA" }], identity: { id: { kind: "integer", value: "1" } }, original: {}, kind: "existing" }], selected: { row: "blob", column: 0 } });
  await key("enter"); assert.equal(host.find("database-cell-editor"), undefined, "Blobs cannot be edited");
  // Sparse insert has no concrete value: DEFAULT is not SQL NULL or empty text.
  update({ rows: [{ key: "new:1", values: [], identity: null, original: {}, kind: "insert" }], selected: { row: "new:1", column: 0 } }); await pump();
  await click(find("database-cell:new:1:0")); await click(find("database-cell:new:1:0")); find("database-cell-editor");
  await click(label("Stage value")); assert.equal(edits.at(-1), undefined);
  await key("enter"); await click(label("NULL")); await click(label("Stage value")); assert.deepEqual(edits.at(-1), { kind: "null" });
  await key("enter"); await click(label("text")); await click(label("Stage value")); assert.deepEqual(edits.at(-1), { kind: "text", value: "" });
  await key("enter"); await click(label("DEFAULT")); await click(label("Stage value")); assert.equal(edits.at(-1), undefined);
  await key("enter"); await click(label("integer"));
  root.dispatch({ elementId: find("database-cell-value").id, eventType: "change", value: "1.5" }); await pump();
  const count = edits.length; await click(label("Stage value")); assert.equal(edits.length, count, "Invalid integers never stage"); find("database-cell-editor"); await key("escape");
  // Filter selection stacks independently, with native search input and counts.
  await click(find("database-filter:col0")); await pump(); find("database-filter-search");
  root.dispatch({ elementId: find("database-filter-search").id, eventType: "change", value: "Italy" }); await pump();
  assert.equal([...host.nodes.values()].filter(node => node.props.role === "option").length, 1, "Unique value search filters the options");
  const italy = [...host.nodes.values()].find(node => node.props.role === "option" && node.props["aria-label"] === "Italy"); assert.ok(italy); await click(italy);
  assert.equal(model.state.filters.col0?.length, 1);
  await key("escape"); await click(find("database-filter:col1")); await pump();
  const nullValue = [...host.nodes.values()].find(node => node.props.role === "option" && node.props["aria-label"] === "NULL"); assert.ok(nullValue); await click(nullValue);
  assert.deepEqual(Object.keys(model.state.filters), ["col0", "col1"]); await key("escape");
  update({ rows: [{ key: "row:0", values: [{ kind: "text", value: "Grape" }], identity: { id: { kind: "integer", value: "0" } }, original: {}, kind: "existing" }], selected: { row: "row:0", column: 0 }, pending: 0 }); await pump();
  await click(label("Delete")); assert.equal(applies, 0); assert.equal(model.state.rows[0]?.kind, "delete");
  await key("s", { ctrl: true }); find("database-delete-confirm"); assert.equal(applies, 0, "Ctrl+S requests confirmation instead of the global file-save action"); assert.equal(interceptedSaves, 0);
  await click(label("Apply 1 (−1)")); assert.equal(applies, 0, "Repeating Apply is not explicit confirmation");
  await click(label("Confirm apply")); assert.equal(applies, 1);
  root.unmount(); assert.ok(unsubscribed, "View unsubscribes without owning controller disposal"); assert.equal(registered, false, "Sidebar unregisters native focus host");
  console.log("Database workbench behavior verified");
} finally { root.unmount(); }