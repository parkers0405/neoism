import { createEffect, createMemo, createSelector, createSignal, For, onCleanup, Show } from "solid-js";
import { useGpuixRequired, type EventPayload, type HostElement, type KeyEvent, type StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { DatabaseController } from "./DatabaseController";
import { parseCell, sameValue, valueText, type DatabaseState, type DatabaseValue, type GridRow } from "./types";
import { handleApplicationShortcut } from "../commands/application-shortcuts";
import { focusNativeElement, invalidateNativeFocus } from "../native-focus";
import { writeClipboard } from "../clipboard";
import { DatabaseScrollbar } from "./DatabaseScrollbar";

type Editor = { row: string; column: number; kind: Exclude<DatabaseValue["kind"], "blob"> | "default"; text: string };
type Filter = { column: string; values: { value: DatabaseValue; count: number }[]; truncated: boolean; loading: boolean };

/** Native GPUIX workbench. The model owns initialization, persistence and lifetime. */
export function DatabaseView(props: {
  controller: DatabaseController;
  config: AppConfig;
  active?: boolean;
  focusToken?: number;
  onFocus?: () => void;
  onCustomKey?: (event: KeyEvent) => boolean;
}) {
  const renderer = useGpuixRequired();
  const [state, setState] = createSignal<DatabaseState>(props.controller.state, { equals: false });
  const [size, setSize] = createSignal({ width: 0, height: 0 });
  const [scroll, setScroll] = createSignal({ x: 0, y: 0 });
  const [filter, setFilter] = createSignal<Filter>();
  const [query, setQuery] = createSignal("");
  const [editor, setEditor] = createSignal<Editor>();
  const [localError, setLocalError] = createSignal("");
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [pageSizeOpen, setPageSizeOpen] = createSignal(false);
  let host: HostElement | undefined, grid: HostElement | undefined, disposed = false, filterRequest = 0;
  let previousTable: string | undefined;
  let lastClick: { row: string; column: number; at: number } | undefined;
  const rows = createMemo(() => state().rows), columns = createMemo(() => state().columns);
  const indexedRows = createMemo<{ row: GridRow; index: number }[]>(previous => {
    const cached = new Map(previous.map(entry => [entry.row, entry]));
    return rows().map((row, index) => { const entry = cached.get(row); return entry?.index === index ? entry : { row, index }; });
  }, []);
  const indexedColumns = createMemo(() => columns().map((name, index) => ({ name, index })));
  const scrollX = createMemo(() => scroll().x), scrollY = createMemo(() => scroll().y);
  const selectedColumn = createMemo(() => state().selected?.column);
  const selectedRowKey = createSelector(() => state().selected?.row);
  const offset = createMemo(() => state().offset);
  const scale = () => props.config.scale;
  const palette = () => props.config.theme;
  const rowHeight = () => 25 * scale(), headerHeight = () => 30 * scale(), gutter = () => 38 * scale();
  const columnWidth = () => Math.max(150 * scale(), (size().width - gutter()) / Math.max(1, Math.min(6, columns().length)));
  const contentWidth = () => gutter() + columns().length * columnWidth();
  const contentHeight = () => rows().length * rowHeight();
  const verticalScrollbar = () => size().height > 0 && contentHeight() > size().height;
  const horizontalScrollbar = () => size().width > 0 && contentWidth() > size().width + 1;
  const moveTo = (x:number,y:number) => setScroll(old=>{
    x=Math.max(0,Math.min(Math.max(0,contentWidth()-size().width),x));
    y=Math.max(0,Math.min(Math.max(0,contentHeight()-size().height),y));
    return old.x===x&&old.y===y?old:{x,y};
  });
  const moveScrollbar = (axis: "horizontal" | "vertical", offset: number) => moveTo(axis==="horizontal"?offset:scrollX(),axis==="vertical"?offset:scrollY());
  const wheel = (event:EventPayload) => {
    const dx=event.modifiers?.shift?-(event.deltaY||0):-(event.deltaX||0);
    const dy=event.modifiers?.shift?0:-(event.deltaY||0);
    moveTo(scrollX()+dx,scrollY()+dy);
  };
  createEffect(()=>{contentWidth();contentHeight();size();moveTo(scrollX(),scrollY());});
  const busy = () => state().loading || state().applying;
  const pageLocked = () => busy() || state().pending > 0 || !!editor();
  const text = (color = palette().fg): StyleDesc => ({
    fontFamily: props.config.fontFamily, fontWeight: props.config.fontWeight, fontSize: 12 * scale(),
    color, whiteSpace: "nowrap", textOverflow: "ellipsis", overflow: "hidden",
  });
  const line = (): StyleDesc => ({ borderColor: palette().border, borderBottomWidth: scale() });
  const horizontal = (): StyleDesc => ({ display: "flex", flexDirection: "row", alignItems: "center", minWidth: 0 });
  const report = (cause: unknown) => { if (!disposed) setLocalError(cause instanceof Error ? cause.message : String(cause)); };
  const run = (operation: () => Promise<unknown>) => {
    setLocalError("");
    try { void operation().catch(report); } catch (cause) { report(cause); }
  };
  const focus = (force = false) => {
    if (force && props.active === false) props.onFocus?.();
    queueMicrotask(() => {
      if (disposed || !force && props.active === false || !host?.mounted || editor() || filter()) return;
      host.root?.mutations.flushMutations();
      focusNativeElement(renderer, host.id);
    });
  };
  createEffect(() => {
    const controller = props.controller;
    setState(controller.state);
    onCleanup(controller.subscribe(() => setState(controller.state)));
  });
  createEffect(() => { props.focusToken; if (props.active !== false) focus(); });
  createEffect(() => {
    if (state().table === previousTable) return;
    previousTable = state().table;
    filterRequest++;
    setFilter(undefined); setEditor(undefined); setConfirmDelete(false); setPageSizeOpen(false); setScroll({ x: 0, y: 0 });
  });
  onCleanup(() => { disposed = true; filterRequest++; });

  const selectedRow = () => state().rows.find(row => row.key === state().selected?.row);
  const deletionCount = () => state().rows.filter(row => row.kind === "delete").length;
  const closeFilter = () => { filterRequest++; setFilter(undefined); focus(); };
  const openFilter = (column: string) => {
    if (filter()?.column === column) { closeFilter(); return; }
    const request = ++filterRequest;
    setQuery(""); setFilter({ column, values: [], truncated: false, loading: true });
    run(async () => {
      try {
        const result = await props.controller.loadValues(column);
        if (!disposed && request === filterRequest) setFilter({ column, ...result, loading: false });
      } catch (cause) {
        if (!disposed && request === filterRequest) { setFilter(old => old ? { ...old, loading: false } : old); throw cause; }
      }
    });
  };
  const filteredValues = createMemo(() => filter()?.values.filter(item => valueText(item.value).toLowerCase().includes(query().toLowerCase())) || []);
  const toggleValue = (column: string, value: DatabaseValue) => {
    // Undefined means unrestricted; an empty array deliberately matches no rows.
    const selected = state().filters[column] ?? filter()?.values.map(item => item.value) ?? [];
    const next = selected.some(item => sameValue(item, value)) ? selected.filter(item => !sameValue(item, value)) : [...selected, value];
    run(() => props.controller.setFilter(column, next));
  };
  const beginEdit = (row: GridRow, column: number) => {
    if (!state().editable || busy() || row.kind === "delete" || row.kind !== "insert" && !row.identity) return;
    const value = row.values[column];
    if (value?.kind === "blob") { setLocalError("Binary cells are read-only"); return; }
    const name = state().columns[column];
    const isDefault = row.kind === "insert" && !(name in row.original) && value === undefined;
    setLocalError("");
    setEditor({ row: row.key, column, kind: isDefault ? "default" : value?.kind ?? "null", text: value && value.kind !== "null" ? String(value.value) : "" });
  };
  const saveEditor = () => {
    const edit = editor(); if (!edit || busy()) return;
    try {
      props.controller.editCell(edit.row, edit.column, edit.kind === "default" ? undefined : parseCell(edit.text, edit.kind));
      setEditor(undefined); setLocalError(""); focus();
    } catch (cause) { report(cause); }
  };
  const apply = (confirmed = false) => {
    if (!state().pending || busy()) return;
    if (deletionCount() && !confirmed) { setConfirmDelete(true); return; }
    setConfirmDelete(false); run(() => props.controller.apply());
  };
  const reveal = (row: number, column: number) => {
    const current = scroll(), top = row * rowHeight(), left = gutter() + column * columnWidth();
    const y = Math.max(0, top < current.y ? top : top + rowHeight() > current.y + size().height ? top + rowHeight() - size().height : current.y);
    const x = Math.max(0, left < current.x + gutter() ? left - gutter() : left + columnWidth() > current.x + size().width ? left + columnWidth() - size().width : current.x);
    moveTo(x,y);
  };
  const keyDown = (event: KeyEvent) => {
    const key = event.key?.toLowerCase().replace(/^arrow/, "");
    const ctrl = event.modifiers?.ctrl || event.modifiers?.cmd, alt = event.modifiers?.alt;
    const localNavigation = !ctrl && !alt && ["up", "down", "left", "right", "home", "end", "pageup", "pagedown", "tab", "enter", "escape"].includes(key || "");
    const localShortcut = !editor() && !filter() && !alt && !event.modifiers?.shift && ctrl && (key === "c" || key === "s");
    if (handleApplicationShortcut(renderer, event) || !(localNavigation || localShortcut) && props.onCustomKey?.(event)) return;
    if (key === "escape") {
      setEditor(undefined); closeFilter(); setConfirmDelete(false); event.preventDefault(); event.stopPropagation(); return;
    }
    if (editor() || filter() || !localNavigation && !localShortcut) return;
    const row = selectedRow(), column = state().selected?.column ?? 0;
    if ((event.modifiers?.ctrl || event.modifiers?.cmd) && key === "c" && row) {
      const value = row.values[column];
      run(() => writeClipboard(value ? valueText(value) : "DEFAULT"));
    } else if ((event.modifiers?.ctrl || event.modifiers?.cmd) && key === "s") apply();
    else if (key === "enter" && row) beginEdit(row, column);
    else if (["up", "down", "left", "right", "home", "end", "pageup", "pagedown", "tab"].includes(key || "")) {
      if (!state().rows.length || !state().columns.length) return;
      let index = Math.max(0, state().rows.findIndex(item => item.key === row?.key)), next = column;
      const step = Math.max(1, Math.floor(size().height / rowHeight()));
      if (key === "up") index--; if (key === "down") index++;
      if (key === "left") next--; if (key === "right") next++;
      if (key === "pageup") index -= step; if (key === "pagedown") index += step;
      if (key === "home") next = 0; if (key === "end") next = state().columns.length - 1;
      if (key === "tab") { next += event.modifiers?.shift ? -1 : 1; if (next >= state().columns.length) { next = 0; index++; } if (next < 0) { next = state().columns.length - 1; index--; } }
      index = Math.max(0, Math.min(state().rows.length - 1, index)); next = Math.max(0, Math.min(state().columns.length - 1, next));
      props.controller.selectCell(state().rows[index]!.key, next); reveal(index, next);
    } else return;
    event.preventDefault(); event.stopPropagation();
  };
  // The virtual window and all three panes commit from the same offset. Native
  // scrolling must not move the data a frame before the frozen panes catch up.
  const rowBuffer = createMemo(() => Math.max(8, Math.ceil(size().height / rowHeight() / 2)));
  const firstRow = createMemo(() => Math.max(0, Math.floor(scrollY() / rowHeight()) - rowBuffer()));
  const lastRow = createMemo(() => Math.ceil((scrollY() + size().height) / rowHeight()) + rowBuffer());
  const firstColumn = createMemo(() => Math.max(0, Math.floor(scrollX() / columnWidth()) - 1));
  const visibleRows = createMemo(() => indexedRows().slice(firstRow(), lastRow()));
  const visibleColumns = createMemo(() => indexedColumns().slice(firstColumn(), firstColumn() + Math.ceil(size().width / columnWidth()) + 3));
  const popupWidth = () => Math.min(270 * scale(), Math.max(0, size().width - 16 * scale()));
  const popupLeft = () => Math.max(8 * scale(), Math.min(size().width - popupWidth() - 8 * scale(), gutter() + (columns().indexOf(filter()?.column || "") + 1) * columnWidth() - scroll().x - popupWidth()));

  function Action(button: { label: string; accessibleLabel?: string; onClick: () => void; disabled?: boolean; accent?: boolean; id?: string; icon?: "filter" }) {
    return <div role="button" aria-label={button.accessibleLabel || button.label} aria-description={button.disabled ? "Unavailable" : undefined} tabIndex={button.disabled ? undefined : 0} testId={button.id}
      onClick={() => { if (!button.disabled) button.onClick(); }}
      onKeyDown={event => {
        if (handleApplicationShortcut(renderer, event) || props.onCustomKey?.(event)) return;
        if (event.key === "enter" || event.key === "space" || event.key === " ") { event.preventDefault(); event.stopPropagation(); if (!button.disabled) button.onClick(); }
      }}
      style={{ ...horizontal(), height: 25 * scale(), paddingLeft: 7 * scale(), paddingRight: 7 * scale(), flexShrink: 0,
        cursor: button.disabled ? "default" : "pointer", opacity: button.disabled ? 0.45 : 1, hover: { backgroundColor: button.disabled ? undefined : palette().hover } }}>
      <Show when={button.icon === "filter"} fallback={<text style={{ ...text(button.accent ? palette().accent : palette().muted), pointerEvents: "none" }}>{button.label}</text>}>
        <svg source={`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="${button.accent ? palette().accent : palette().muted}" stroke-width="1.2" stroke-linejoin="round"><path d="M2 3h12L9.5 8v4.5l-3 1V8z"/></svg>`} style={{ width: 13 * scale(), height: 13 * scale(), pointerEvents: "none" }} />
      </Show>
    </div>;
  }
  return <div ref={element => host = element} role="region" aria-label="Database workbench" tabIndex={0} testId="database-workbench"
    onFocus={() => { invalidateNativeFocus(renderer); props.onFocus?.(); }} onBlur={() => invalidateNativeFocus(renderer)} onKeyDown={keyDown}
    style={{ ...horizontal(), alignItems: "stretch", position: "relative", width: "100%", height: "100%", minHeight: 0, overflow: "hidden", backgroundColor: palette().bg }}>
    <div style={{ display: "flex", flexDirection: "column", flexGrow: 1, flexBasis: 0, minWidth: 0, minHeight: 0, position: "relative", overflow: "hidden" }}>
      <Show when={state().error || localError() || state().notice}>
        <div style={{ ...horizontal(), ...line(), minHeight: 27 * scale(), paddingLeft: 8 * scale(), backgroundColor: palette().surface }}>
          <text style={{ ...text(state().error || localError() ? palette().accent : palette().muted), flexGrow: 1, flexBasis: 0 }}>{state().error || localError() || state().notice}</text>
          <Action label="×" onClick={() => { setLocalError(""); props.controller.clearError(); }} />
        </div>
      </Show>
      <Show when={state().table} fallback={<div style={{ flexGrow: 1, flexBasis: 0, padding: 18 * scale() }}><text style={text(palette().muted)}>{state().loading ? "Loading schema…" : "Select a table or view"}</text></div>}>
        <div role="row" testId="database-header" onScroll={wheel} style={{ ...line(), height: headerHeight(), minHeight: headerHeight(), position: "relative", overflow: "hidden", backgroundColor: palette().surface }}>
          <div testId="database-header-plane" style={{position:"absolute",left:-scrollX(),top:0,width:contentWidth(),height:headerHeight(),pointerEvents:"none"}}>
            <For each={visibleColumns()}>{column => <div role="columnheader" style={{ ...horizontal(), position: "absolute", left: gutter() + column.index * columnWidth(), top: 0, width: columnWidth(), height: headerHeight(), borderRightWidth: scale(), borderColor: palette().border }}>
            <div role="button" aria-label={`Sort ${column.name}`} onClick={() => run(() => props.controller.sortColumn(column.name))} style={{ flexGrow: 1, flexBasis: 0, minWidth: 0, paddingLeft: 9 * scale(), cursor: "pointer" }}>
              <text style={{ ...text(), pointerEvents: "none" }}>{`${column.name}${state().sort?.column === column.name ? state().sort?.descending ? " ↓" : " ↑" : ""}`}</text>
            </div>
            <Action label="Filter" icon="filter" accessibleLabel={`Filter ${column.name}`} disabled={busy()} accent={state().filters[column.name] !== undefined} id={`database-filter:${column.name}`} onClick={() => openFilter(column.name)} />
          </div>}</For>
          </div>
          <div style={{ position: "absolute", left: 0, top: 0, width: gutter(), height: headerHeight(), backgroundColor: palette().surface, borderRightWidth: scale(), borderColor: palette().border, padding: 8 * scale() }}><text style={text(palette().muted)}>#</text></div>
          <Show when={verticalScrollbar()}><div style={{position:"absolute",right:0,top:0,width:12*scale(),height:headerHeight(),backgroundColor:palette().surface,pointerEvents:"none"}}/></Show>
        </div>
        <div style={{display:"flex",flexDirection:"row",flexGrow:1,flexBasis:0,minHeight:0,minWidth:0}}>
        <div ref={element => grid = element} role="grid" aria-label={state().table} testId="database-grid"
          onMouseDown={() => focus(true)}
          onLayout={event => setSize(old => event.width === old.width && event.height === old.height ? old : { width: event.width || 0, height: event.height || 0 })}
          onScroll={wheel}
          style={{ flexGrow: 1, flexBasis: 0, minHeight: 0, minWidth: 0, overflow: "hidden", position: "relative", backgroundColor: palette().bg }}>
          <div testId="database-data-plane" style={{ left:-scrollX(),top:-scrollY(),width: Math.max(size().width, contentWidth()), height: Math.max(size().height,contentHeight()), position: "absolute", pointerEvents:"none" }}>
            <For each={visibleColumns()}>{column => <div style={{ position: "absolute", left: gutter() + column.index * columnWidth(), top: 0, width: columnWidth(), height: "100%", borderRightWidth: scale(), borderColor: palette().border, pointerEvents: "none" }} />}</For>
            <For each={visibleRows()}>{entry => <div role="row" aria-selected={selectedRowKey(entry.row.key)} testId={`database-row:${entry.row.key}`}
              style={{ position: "absolute", top: entry.index * rowHeight(), left: 0, width: contentWidth(), height: rowHeight(), ...line(), backgroundColor: entry.row.kind === "delete" || selectedRowKey(entry.row.key) ? palette().hover : undefined }}>
              <For each={visibleColumns()}>{column => {
                const value = () => entry.row.values[column.index];
                const selected = createMemo(() => selectedRowKey(entry.row.key) && selectedColumn() === column.index);
                return <div role="gridcell" aria-selected={selected()} testId={`database-cell:${entry.row.key}:${column.index}`}
                  onClick={() => {
                    props.controller.selectCell(entry.row.key, column.index);
                    const now = Date.now();
                    if (lastClick?.row === entry.row.key && lastClick.column === column.index && now - lastClick.at < 350) { lastClick = undefined; beginEdit(entry.row, column.index); }
                    else { lastClick = { row: entry.row.key, column: column.index, at: now }; focus(true); }
                  }}
                  style={{ position: "absolute", left: gutter() + column.index * columnWidth(), top: 0, width: columnWidth(), height: rowHeight(), borderRightWidth: scale(), borderColor: palette().border,
                    paddingLeft: 9 * scale(), paddingTop: 4 * scale(), overflow: "hidden", opacity: entry.row.kind === "delete" ? 0.4 : 1 }}>
                  <text style={{ ...text(!value() || value()?.kind === "null" ? palette().muted : palette().fg), textDecoration: entry.row.kind === "delete" ? "line-through" : "none", pointerEvents: "none" }}>{value() ? valueText(value()!) : "DEFAULT"}</text>
                  <Show when={selected()}><div testId="database-cell-selection" style={{position:"absolute",left:0,top:0,width:columnWidth()-scale(),height:rowHeight()-scale(),borderWidth:Math.max(1,scale()),borderColor:palette().accent,pointerEvents:"none"}} /></Show>
                </div>;
              }}</For>
            </div>}</For>
          </div>
          <div testId="database-gutter-plane" style={{position:"absolute",left:0,top:-scrollY(),width:gutter(),height:Math.max(size().height,contentHeight()),backgroundColor:palette().surface,pointerEvents:"none",borderRightWidth:scale(),borderColor:palette().border}}>
            <For each={visibleRows()}>{entry=><div role="rowheader" testId={`database-gutter:${entry.row.key}`} onClick={() => { props.controller.selectCell(entry.row.key, state().selected?.column ?? 0); focus(true); }}
                style={{ position: "absolute", left:0, top: entry.index*rowHeight(), width: gutter(), height: rowHeight(),borderBottomWidth:scale(), borderColor: palette().border, backgroundColor: palette().surface, paddingTop: 4 * scale(), paddingRight: 7 * scale() }}>
                <text style={{ ...text(entry.row.kind === "existing" ? palette().muted : palette().accent), textAlign: "right", pointerEvents: "none" }}>{entry.row.kind === "delete" ? "−" : entry.row.kind === "insert" ? "+" : entry.row.kind === "update" ? "•" : String(offset() + entry.index + 1)}</text>
            </div>}</For>
          </div>
        </div>
        <Show when={verticalScrollbar()}><DatabaseScrollbar axis="vertical" config={props.config} viewport={size().height} content={contentHeight()} offset={scrollY()} onChange={value=>moveScrollbar("vertical",value)} onWheel={wheel}/></Show>
        </div>
        <Show when={horizontalScrollbar()}><div style={{display:"flex",flexDirection:"row",height:12*scale(),minHeight:12*scale(),flexShrink:0,backgroundColor:palette().surface}}>
          <div style={{width:gutter(),flexShrink:0,borderRightWidth:scale(),borderColor:palette().border}}/>
          <DatabaseScrollbar axis="horizontal" config={props.config} viewport={Math.max(0,size().width-gutter())} content={Math.max(0,contentWidth()-gutter())} offset={scrollX()} onChange={value=>moveScrollbar("horizontal",value)} onWheel={wheel}/>
          <Show when={verticalScrollbar()}><div style={{width:12*scale(),flexShrink:0}}/></Show>
        </div></Show>
      </Show>
      <Show when={Object.keys(state().filters).length > 0}>
        <div style={{ ...horizontal(), ...line(), minHeight: 26 * scale(), overflowX: "scroll", backgroundColor: palette().surface }}>
          <For each={Object.entries(state().filters)}>{([column, values]) => <Action label={`${column}: ${values.length} selected ×`} accent onClick={() => run(() => props.controller.setFilter(column, undefined))} />}</For>
        </div>
      </Show>
      <Show when={confirmDelete()}><div testId="database-delete-confirm" role="alert" style={{ ...horizontal(), ...line(), minHeight: 30 * scale(), paddingLeft: 8 * scale(), backgroundColor: palette().surface }}>
        <text style={{ ...text(), flexGrow: 1, flexBasis: 0 }}>{`Permanently delete ${deletionCount()} row(s) and apply all ${state().pending} changes?`}</text>
        <Action label="Confirm apply" accent onClick={() => apply(true)} disabled={busy()} /><Action label="Cancel" onClick={() => setConfirmDelete(false)} />
      </div></Show>
      <Show when={state().editable}>
        <div testId="database-row-actions" style={{ ...horizontal(), minHeight: 30 * scale(), flexShrink: 0, borderTopWidth: scale(), borderColor: palette().border, paddingLeft: 8 * scale(), paddingRight:8*scale(), backgroundColor: palette().surface }}>
          <Action label="New row" id="database-new-row" disabled={busy()} onClick={() => { props.controller.addRow(); setConfirmDelete(false); const index = state().rows.length - 1; if (index >= 0) reveal(index, 0); focus(true); }} />
          <Action label="Delete" disabled={busy() || !selectedRow() || selectedRow()?.kind === "delete" || selectedRow()?.kind !== "insert" && !selectedRow()?.identity} onClick={() => { const row = selectedRow(); if (row) props.controller.deleteRow(row.key); setConfirmDelete(false); }} />
          <div style={{flexGrow:1,minWidth:0}} />
          <Action label={`Apply ${state().pending}${deletionCount() ? ` (−${deletionCount()})` : ""}`} accent disabled={busy() || !state().pending} onClick={apply} />
          <Action label="Discard" disabled={busy() || !state().pending} onClick={() => { props.controller.discard(); setConfirmDelete(false); setEditor(undefined); }} />
        </div>
      </Show>
      <div testId="database-footer" style={{ ...horizontal(), height: 30 * scale(), minHeight: 30 * scale(), borderTopWidth: scale(), borderColor: palette().border, paddingLeft: 15 * scale(), paddingRight:8*scale(), backgroundColor: palette().surface, overflow:"hidden" }}>
        <text testId="database-rows-label" style={{ ...text(palette().muted), flexGrow:1,flexBasis:0,minWidth:0,marginRight:6*scale() }}>{`Rows ${state().rows.some(row => row.kind !== "insert") ? state().offset + 1 : 0}–${Math.min(state().offset + state().rows.filter(row => row.kind !== "insert").length, state().total)} of ${state().total}${busy()?" · Working…":!state().editable?" · Read-only":""}`}</text>
        <Action label={`${state().limit} / page`} accessibleLabel="Rows per page" id="database-page-size" disabled={pageLocked()} onClick={()=>setPageSizeOpen(!pageSizeOpen())}/>
        <Action label="Prev" accessibleLabel="Previous page" id="database-page-prev" disabled={pageLocked() || state().offset === 0} onClick={() => run(() => props.controller.page(-1))} />
        <text testId="database-page-number" style={{...text(palette().muted),flexShrink:0,marginLeft:4*scale(),marginRight:4*scale()}}>{`Page ${Math.floor(state().offset/state().limit)+1} / ${Math.max(1,Math.ceil(state().total/state().limit))}`}</text>
        <Action label="Next" accessibleLabel="Next page" id="database-page-next" disabled={pageLocked() || state().offset + state().limit >= state().total} onClick={() => run(() => props.controller.page(1))} />
        <Action label="↻" accessibleLabel="Refresh table" disabled={pageLocked()} onClick={() => run(() => props.controller.refresh())} />
      </div>
      <Show when={pageSizeOpen()}><div role="menu" aria-label="Rows per page" onMouseDownOutside={()=>setPageSizeOpen(false)} style={{position:"absolute",right:8*scale(),bottom:32*scale(),minWidth:130*scale(),padding:4*scale(),backgroundColor:palette().surface,borderWidth:scale(),borderColor:palette().border,borderRadius:4*scale()}}>
        <For each={[25,50,100,200]}>{limit=><Action label={`${limit} rows per page`} accent={state().limit===limit} onClick={()=>{setPageSizeOpen(false);run(()=>props.controller.setPageSize(limit));}}/>}</For>
      </div></Show>
      <Show when={filter()}>{current => <div role="dialog" aria-label={`Filter ${current().column}`} testId="database-filter-popover" onMouseDownOutside={closeFilter}
        style={{ position: "absolute", left: popupLeft(), top: headerHeight() + (state().error || localError() || state().notice ? 27 * scale() : 0), width: popupWidth(), maxHeight: Math.max(100 * scale(), size().height), backgroundColor: palette().surface, borderWidth: scale(), borderColor: palette().border, borderRadius: 6 * scale(), padding: 6 * scale(), overflow: "hidden" }}>
        <div style={horizontal()}><text style={{ ...text(), flexGrow: 1, flexBasis: 0 }}>{current().column}</text><Action label="Clear all" disabled={busy()} onClick={() => run(() => props.controller.setFilter(current().column, undefined))} /><Action label="×" onClick={closeFilter} /></div>
        <input autoFocus value={query()} placeholder="Search unique values" aria-label="Search unique values" testId="database-filter-search" onChange={event => setQuery(event.value || "")}
          onKeyDown={event => { if (handleApplicationShortcut(renderer, event) || props.onCustomKey?.(event)) return; event.stopPropagation(); if (event.key === "escape") { event.preventDefault(); closeFilter(); } }}
          style={{ ...text(), height: 27 * scale(), width: "100%", backgroundColor: palette().bg, borderWidth: scale(), borderColor: palette().border, marginBottom: 4 * scale() }} />
        <div style={{ maxHeight: Math.max(50 * scale(), Math.min(250 * scale(), size().height - 100 * scale())), overflowY: "scroll" }}>
          <Show when={current().loading}><text style={text(palette().muted)}>Loading values…</text></Show>
          <For each={filteredValues()}>{item => <div role="option" aria-label={valueText(item.value)} aria-selected={state().filters[current().column]?.some(value => sameValue(value, item.value)) ?? true}
            onClick={() => { if (!busy()) toggleValue(current().column, item.value); }} style={{ ...horizontal(), height: 25 * scale(), cursor: "pointer", hover: { backgroundColor: palette().hover } }}>
            <text style={{ ...text(palette().accent), width: 23 * scale(), pointerEvents: "none" }}>{(state().filters[current().column]?.some(value => sameValue(value, item.value)) ?? true) ? "✓" : ""}</text>
            <text style={{ ...text(), flexGrow: 1, flexBasis: 0, pointerEvents: "none" }}>{valueText(item.value)}</text><text style={{ ...text(palette().muted), pointerEvents: "none" }}>{String(item.count)}</text>
          </div>}</For>
          <Show when={!current().loading && !filteredValues().length}><text style={text(palette().muted)}>No matching values</text></Show>
        </div>
        <Show when={current().truncated}><text style={text(palette().muted)}>Value list truncated; selection is explicit.</text></Show>
      </div>}</Show>
      <Show when={editor()}>{edit => <div role="dialog" aria-label="Edit cell" testId="database-cell-editor" style={{ position: "absolute", left: 8 * scale(), right: 8 * scale(), bottom: 36 * scale(), padding: 8 * scale(), borderWidth: scale(), borderColor: palette().border, backgroundColor: palette().surface }}>
        <div style={horizontal()}><text style={{ ...text(), flexGrow: 1, flexBasis: 0 }}>{state().columns[edit().column]}</text>
          <For each={(["text", "integer", "real", "null", ...(state().rows.find(row => row.key === edit().row)?.kind === "insert" ? ["default"] : [])] as Editor["kind"][])}>{kind => <Action label={kind === "default" ? "DEFAULT" : kind === "null" ? "NULL" : kind} accent={edit().kind === kind} disabled={busy()} onClick={() => setEditor(old => old ? { ...old, kind } : old)} />}</For>
        </div>
        <input autoFocus value={edit().text} readOnly={busy() || edit().kind === "null" || edit().kind === "default"} aria-label="Cell value" testId="database-cell-value" placeholder={edit().kind === "text" ? "Empty string is a value" : edit().kind === "default" ? "Use column default" : edit().kind === "null" ? "SQL NULL" : "Number"}
          onChange={event => setEditor(old => old ? { ...old, text: event.value || "" } : old)} onSubmit={saveEditor}
          onKeyDown={event => { if (handleApplicationShortcut(renderer, event) || props.onCustomKey?.(event)) return; event.stopPropagation(); if (event.key === "escape") { event.preventDefault(); setEditor(undefined); focus(); } }}
          style={{ ...text(), height: 28 * scale(), width: "100%", backgroundColor: palette().bg, borderWidth: scale(), borderColor: palette().border }} />
        <div style={{ ...horizontal(), justifyContent: "flex-end" }}><Action label="Cancel" onClick={() => { setEditor(undefined); focus(); }} /><Action label="Stage value" accent disabled={busy()} onClick={saveEditor} /></div>
      </div>}</Show>
    </div>
  </div>;
}