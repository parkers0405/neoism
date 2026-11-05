import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { useGpuixRequired, type HostElement as GpuixElement, type KeyEvent, type StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { DatabaseController } from "./DatabaseController";
import type { DatabaseState } from "./types";
import { focusNativeElement, invalidateNativeFocus } from "../native-focus";
import { handleApplicationShortcut } from "../commands/application-shortcuts";
import { paintedChromeBounds, type ChromeBounds } from "../ui/chrome-target-bounds";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";

export interface DatabaseSidebarProps {
  controller: DatabaseController | undefined;
  config: AppConfig;
  connections: { id: string; title: string; path: string }[];
  activeId?: string;
  onConnection: (id: string) => void;
  onTable: (name: string) => void;
  onBrowse: () => void;
  active?: boolean;
  focusToken?: number;
  onFocus?: () => void;
  onCustomKey?: (event: KeyEvent) => boolean;
  registerHost?: (host: GpuixElement | undefined) => void;
  onBounds?: (host:GpuixElement,bounds:{x:number;y:number;width:number;height:number})=>void;
  onCursor?: (rect:ChromeBounds|undefined,host:GpuixElement,bounds:ChromeBounds)=>void;
}
interface TreeRow {
  key: string; parent?: string; depth: number; label: string; glyph: string;
  open?: boolean; toggle?: () => void; activate?: () => void; testId?: string;
}

/** Workspace sidebar only. Table activation belongs to the app's normal buffer
 * tabs; never navigate the schema controller or a table's independent draft. */
export function DatabaseSidebar(props: DatabaseSidebarProps) {
  const renderer = useGpuixRequired();
  const [state, setState] = createSignal<DatabaseState | undefined>(undefined, { equals: false });
  const [expanded, setExpanded] = createSignal(new Set<string>());
  const [selection, setSelection] = createSignal("");
  const [focused, setFocused] = createSignal(false);
  const rowHosts = new Map<string, GpuixElement>();
  const measurements=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let treeHost:GpuixElement|undefined;
  let host: GpuixElement | undefined, disposed = false, focusQueued = false;
  let publishedCursor="";
  const publishCursor=()=>{
    if(disposed||!host?.mounted||!props.onCursor)return;
    const bounds=paintedChromeBounds(renderer,host.id);
    if(!bounds)return;
    const selected=rowHosts.get(selection()),rect=selected?.mounted?paintedChromeBounds(renderer,selected.id):undefined;
    const viewport=treeHost&&paintedChromeBounds(renderer,treeHost.id);
    const top=rect&&viewport?Math.max(rect.y,viewport.y):0,bottom=rect&&viewport?Math.min(rect.y+rect.height,viewport.y+viewport.height):0;
    const visible=rect&&viewport&&bottom>top?{...rect,y:top,height:bottom-top}:undefined;
    const key=JSON.stringify([selection(),bounds,visible]);
    if(key===publishedCursor)return;
    publishedCursor=key;props.onCursor(visible,host,bounds);
  };
  const measureCursor=()=>{if(props.onCursor&&!disposed)measurements.request(publishCursor);};
  const scale = () => props.config.scale, palette = () => props.config.theme;
  const text = (color = palette().fg): StyleDesc => ({ fontFamily: props.config.fontFamily, fontWeight: props.config.fontWeight,
    fontSize: 12 * scale(), color, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" });
  const row = (): StyleDesc => ({ display: "flex", flexDirection: "row", alignItems: "center", minWidth: 0 });
  const toggle = (key: string) => setExpanded(old => { const next = new Set(old); if (next.has(key)) next.delete(key); else next.add(key); return next; });
  const focus = (force = false) => {
    if (focusQueued) return; focusQueued = true;
    queueMicrotask(() => {
      focusQueued = false;
      if (disposed || !host?.mounted || !force && props.active !== true) return;
      host.root?.mutations.flushMutations(); focusNativeElement(renderer, host.id);
      publishCursor();measureCursor();
    });
  };
  createEffect(() => {
    const controller = props.controller;
    setState(controller?.state);
    if (controller) onCleanup(controller.subscribe(() => setState(() => controller.state)));
  });
  createEffect(() => { props.focusToken; if (props.active) focus(); });
  createEffect(() => { props.activeId; setSelection(props.activeId ? `connection:${props.activeId}` : ""); });
  onCleanup(() => { disposed = true;measurements.dispose(); props.registerHost?.(undefined); });
  const rows = createMemo(() => {
    const result: TreeRow[] = [];
    for (const connection of props.connections) {
      const root = `connection:${connection.id}`, selected = connection.id === props.activeId;
      const open = selected && !expanded().has(`${root}:collapsed`);
      result.push({ key: root, depth: 0, label: connection.title || connection.path.split(/[\\/]/).pop() || connection.path, glyph: "\uf1c0", open,
        toggle: () => { if (!selected) props.onConnection(connection.id); else toggle(`${root}:collapsed`); }, activate: () => props.onConnection(connection.id), testId: `database-connection:${connection.id}` });
      if (!open || !props.controller) continue;
      for (const group of ["Tables", "Views"] as const) {
        const groupKey = `${root}:${group}`, groupOpen = !expanded().has(`${groupKey}:collapsed`);
        const objects = state()?.objects.filter(object => object.kind === (group === "Tables" ? "table" : "view")) || [];
        result.push({ key: groupKey, parent: root, depth: 1, label: `${group}  ${objects.length}`, glyph: "\uf07b", open: groupOpen, toggle: () => toggle(`${groupKey}:collapsed`), testId: `database-group:${group}` });
        if (!groupOpen) continue;
        for (const object of objects) {
          const tableKey = `${root}:table:${object.name}`, tableOpen = expanded().has(tableKey);
          result.push({ key: tableKey, parent: groupKey, depth: 2, label: object.name, glyph: "\uf0ce", open: tableOpen,
            toggle: () => toggle(tableKey), activate: () => props.onTable(object.name), testId: `database-table:${object.name}` });
          if (!tableOpen) continue;
          for (const section of ["Columns", "Indexes", "Triggers"] as const) {
            const key = `${tableKey}:${section}`, sectionOpen = section === "Columns" ? !expanded().has(`${key}:collapsed`) : expanded().has(key);
            const labels = section === "Columns" ? object.columns.map(column => `${column.name}: ${column.data_type || "ANY"}${column.primary_key ? " [PK]" : ""}${column.not_null ? " [NN]" : ""}`)
              : section === "Indexes" ? object.indexes.map(index => `${index.name}${index.unique ? " [UQ]" : ""}`) : object.triggers.map(trigger => trigger.name);
            result.push({ key, parent: tableKey, depth: 3, label: `${section}  ${labels.length}`, glyph: "\uf07b", open: sectionOpen,
              toggle: () => toggle(section === "Columns" ? `${key}:collapsed` : key), testId: `database-section:${object.name}:${section}` });
            if (sectionOpen) labels.forEach((label, index) => result.push({ key: `${key}:${index}`, parent: key, depth: 4, label, glyph: section === "Columns" ? "\uf0db" : "\uf0c1" }));
          }
        }
      }
    }
    return result;
  });
  createEffect(() => {
    const visible = rows();
    if (!visible.some(item => item.key === selection())) setSelection(visible[0]?.key || "");
    measureCursor();
  });
  const select = (key: string) => {
    setSelection(key);
    publishCursor();
    queueMicrotask(() => { const element = rowHosts.get(key); if (!disposed && element?.mounted) { element.root?.mutations.flushMutations(); renderer.scrollIntoView?.(element.id);measureCursor(); } });
  };
  const keyDown = (event: KeyEvent) => {
    const key = event.key?.toLowerCase().replace(/^arrow/, "");
    const modified = event.modifiers?.alt || event.modifiers?.ctrl || event.modifiers?.cmd;
    const local = !modified && ["up", "down", "left", "right", "home", "end", "enter", "space", " "].includes(key || "");
    if (handleApplicationShortcut(renderer, event) || !local && props.onCustomKey?.(event)) return;
    if (modified) return;
    const visible = rows(), index = Math.max(0, visible.findIndex(item => item.key === selection())), current = visible[index];
    if (!current) return;
    if (key === "up" || key === "down" || key === "home" || key === "end") {
      const next = key === "home" ? 0 : key === "end" ? visible.length - 1 : Math.max(0, Math.min(visible.length - 1, index + (key === "up" ? -1 : 1)));
      select(visible[next]!.key);
    } else if (key === "right") { if (current.open === false) current.toggle?.(); else if (visible[index + 1]?.parent === current.key) select(visible[index + 1]!.key); }
    else if (key === "left") { if (current.open) current.toggle?.(); else if (current.parent) select(current.parent); }
    else if (key === "enter" || key === "space" || key === " ") (current.activate || current.toggle)?.();
    else return;
    event.preventDefault(); event.stopPropagation();
  };
  return <div ref={element => { host = element; props.registerHost?.(element); }} role="region" aria-label="Databases" testId="database-sidebar" tabIndex={0}
    onMouseDown={() => { props.onFocus?.(); focus(true); }} onFocus={() => { invalidateNativeFocus(renderer); setFocused(true); props.onFocus?.();publishCursor();measureCursor(); }}
    onBlur={() => { invalidateNativeFocus(renderer); setFocused(false); }} onKeyDown={keyDown}
    onLayout={event=>{if(host)props.onBounds?.(host,{x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0});measureCursor();}}
    style={{ display: "flex", flexDirection: "column", width: "100%", height: "100%", flexGrow: 1, minWidth: 0, minHeight: 0, overflow: "hidden", backgroundColor: palette().bg, userSelect: "none" }}>
    <div style={{ ...row(), height: 32 * scale(), minHeight: 32 * scale(), paddingLeft: 12 * scale(), paddingRight: 8 * scale(), borderBottomWidth: scale(), borderColor: palette().border }}>
      <text style={{ ...text(), flexGrow: 1 }}>Databases</text>
    </div>
    <div role="button" aria-label="Open database" testId="database-browse" tabIndex={0} onClick={props.onBrowse}
      onKeyDown={event => { if (handleApplicationShortcut(renderer, event) || props.onCustomKey?.(event)) return; if (event.key === "enter" || event.key === "space" || event.key === " ") { event.preventDefault(); event.stopPropagation(); props.onBrowse(); } }}
      style={{ ...row(), height: 28 * scale(), minHeight: 28 * scale(), paddingLeft: 12 * scale(), cursor: "pointer", hover: { backgroundColor: palette().hover } }}>
      <text style={{ ...text(palette().muted), pointerEvents: "none" }}>＋ Open database</text>
    </div>
    <div ref={element=>treeHost=element} role="tree" aria-label="Database schema" testId="database-schema" onLayout={measureCursor} onScrollChange={measureCursor} style={{ flexGrow: 1, flexBasis: 0, minHeight: 0, overflowY: "scroll" }}>
      <For each={rows()}>{item => {
        onCleanup(() => rowHosts.delete(item.key));
        return <div ref={element => rowHosts.set(item.key, element)} onLayout={measureCursor} onMouseDown={() => { props.onFocus?.(); select(item.key); }} role="treeitem" aria-label={item.label} aria-level={item.depth + 1} aria-expanded={item.open} aria-selected={selection() === item.key} testId={item.testId ? `${item.testId}:row` : undefined}
          style={{ ...row(), height: 25 * scale(), minHeight: 25 * scale(), paddingLeft: (6 + 12 * item.depth) * scale(), paddingRight: 8 * scale(), cursor: "pointer",
            backgroundColor: selection() === item.key && (focused() || props.active) ? palette().surface : undefined, hover: { backgroundColor: palette().hover } }}>
          <div role={item.toggle ? "button" : undefined} aria-label={item.toggle ? `Expand ${item.label}` : undefined} testId={item.testId ? `${item.testId}:expand` : undefined}
            onClick={() => { select(item.key); item.toggle?.(); }}
            style={{ ...row(), width: 14 * scale(), height: 25 * scale(), flexShrink: 0 }}>
            <text style={{ ...text(palette().muted), pointerEvents: "none" }}>{item.open === undefined ? "" : item.open ? "⌄" : "›"}</text>
          </div>
          <div testId={item.testId} onClick={() => { select(item.key); (item.activate || item.toggle)?.(); }} style={{ ...row(), flexGrow: 1, flexBasis: 0, height: "100%" }}>
            <text style={{ fontFamily: props.config.iconFontFamily, fontSize: 11 * scale(), color: item.glyph === "\uf07b" ? palette().folder : palette().dim, width: 18 * scale(), flexShrink: 0, pointerEvents: "none" }}>{item.glyph}</text>
            <text style={{ ...text(selection() === item.key ? palette().fg : palette().dim), pointerEvents: "none" }}>{item.label}</text>
          </div>
        </div>;
      }}</For>
      <Show when={!props.connections.length}><div style={{ padding: 14 * scale() }}><text style={{ ...text(palette().muted), whiteSpace: "normal" }}>Open a SQLite database to browse its tables and views.</text></div></Show>
      <Show when={props.connections.length && !props.controller}><div style={{ padding: 14 * scale() }}><text style={{ ...text(palette().muted), whiteSpace: "normal" }}>Select a database to explore its schema.</text></div></Show>
      <Show when={state()?.loading}><text style={{ ...text(palette().muted), padding: 12 * scale() }}>Loading schema…</text></Show>
      <Show when={state()?.error}><text style={{ ...text(palette().accent), padding: 12 * scale(), whiteSpace: "normal" }}>{state()?.error}</text></Show>
    </div>
  </div>;
}