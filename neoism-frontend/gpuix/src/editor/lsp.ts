import { createSignal } from "solid-js";
import type { DaemonConnection } from "../daemon/client";
import { isRecord, variant, type EditorClientMessage } from "../daemon/protocol";
import { NativeEditor, type EditorFrame } from "../native-core";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import type {AppConfig} from "../config";
import type {PaintCommand} from "../ui/PaintLayer";
import {sharePaintCommands} from "./paint-command";
import {openExternalUrl} from "../menu-actions/web-frontend";
export interface CodeTextEdit { start_line: number; start_col: number; end_line: number; end_col: number; text: string }
export interface EditorLspConnection {
  client: DaemonConnection; path: string; root: string; openPaths: () => readonly string[];
  openLocation: (path: string, line: number, col: number) => void | Promise<void>;
  applyEdits: (path: string, edits: CodeTextEdit[]) => void | Promise<void>;
}
export interface LspMenu { kind: "completion" | "actions"; selected?: number; rect: number[]; first: number; visibleRows: number; offset: number;
  rowHeight: number; fontSize: number; padX: number; kindWidth: number; menuWidth: number; animating: boolean;
  items: Array<{ word: string; kind: string; menu: string; info: string }> }
export interface LspHover {line:number;col:number;x:number;y:number;lines:string[];rect:number[];viewport:number[];paintOrigin:number[];paintSize:number[];commands:PaintCommand[];selectedText?:string|null;dragging:boolean;scrollY:number;maxScroll:number;hoveredLink?:string|null}
export interface LspFrame { menu?: LspMenu | null; hover?: LspHover | null;
  diagnostics?: { error: number; warn: number; info: number; hint: number };
  requests: EditorClientMessage[]; events: Array<{ kind: string; message?: string; level?: string; path?: string; line?: number; col?: number; root?: string; rows?: ReferenceRow[]; edits?: CodeTextEdit[]; word?: string;url?:string }> }
export interface ReferenceRow { path: string; line: number; column: number; text: string }
export type LspAction = "completion" | "hover" | "signature" | "definition" | "references" | "actions" | "rename" | "format" | "dismiss";
export function createEditorLsp(editor: NativeEditor, connection: EditorLspConnection, host: { save: () => Promise<void>; report: (error: unknown) => void; focus: () => void }) {
  const [state, setState] = createSignal<LspFrame>();
  const [rename, setRename] = createSignal<string>();
  const [references, setReferences] = createSignal<{ root: string; rows: ReferenceRow[] }>();
  const [notice, setNotice] = createSignal<{ message: string; error: boolean }>();
  const surface = `gpuix:${editor.id}`;
  const pending = new Set<number>();
  let disposed = false, pointerTimer: ReturnType<typeof setTimeout> | undefined, noticeTimer: ReturnType<typeof setTimeout> | undefined;
  const pointerTick = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const hoverTick = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let latestHoverPointer:{x:number;y:number}|undefined;
  let pointerAt = 0;
  let latestPointer: { x: number; y: number } | undefined;
  let pointerInsideHover=false,lastHoverPoint:{x:number;y:number}|undefined;
  const run = (operation: string, args: Record<string, unknown> = {}) => { if (!disposed) { try { editor.call(`lsp.${operation}`, args); } catch (error) { host.report(error); } } };
  const consume = (frame: EditorFrame) => {
    const lsp = frame.lsp; if (!lsp || disposed) return;
    setState(previous=>({...lsp,hover:lsp.hover?{...lsp.hover,commands:sharePaintCommands(previous?.hover?.commands||[],lsp.hover.commands)}:lsp.hover}));
    for (const request of lsp.requests) {
      try { const id = connection.client.send("Editor", request, connection.root); if (!(typeof request === "object" && "DidSave" in request)) pending.add(id); if (pending.size > 512) pending.delete(pending.values().next().value!); }
      catch (error) { host.report(error); }
    }
    for (const event of lsp.events) {
      if (event.kind === "rename") setRename(event.word || "");
      else if (event.kind === "references") setReferences({ root: event.root || connection.root, rows: event.rows || [] });
      else if (event.kind === "location" && event.path) Promise.resolve(connection.openLocation(event.path, event.line || 0, event.col || 0)).catch(host.report);
      else if (event.kind === "edits" && event.path) Promise.resolve(connection.applyEdits(event.path, event.edits || [])).catch(host.report);
      else if (event.kind === "save") void host.save().catch(host.report);
      else if(event.kind==="open-url"&&event.url&&/^https?:\/\//i.test(event.url))void openExternalUrl(event.url).catch(host.report);
      else if (event.kind === "toast" && event.message) {
        setNotice({ message: event.message, error: event.level === "Error" }); clearTimeout(noticeTimer); noticeTimer = setTimeout(() => setNotice(undefined), 4000);
      }
    }
    if (lsp.menu?.animating) editor.requestFrame();
  };
  const unsubscribeEditor = editor.subscribe(consume);
  const unsubscribeDaemon = connection.client.subscribe((event) => {
    if (event.family !== "Editor" || disposed) return;
    const match = variant(event.message); if (!match || !isRecord(match[1])) return;
    const body = match[1];
    if (typeof body.surface_id === "string" ? body.surface_id !== surface
      : typeof body.file_path === "string" ? body.file_path !== connection.path : !pending.has(event.requestId)) return;
    pending.delete(event.requestId);
    if (match[0] === "Batch" && Array.isArray(body.messages)) { for (const message of body.messages) run("reply", { message }); }
    else run("reply", { message: event.message });
  });
  const unsubscribeStatus = connection.client.subscribeStatus((status) => {
    if (disposed) return;
    pending.clear();
    if (status.phase === "connected") run("enable", { surface, path: connection.path, openPaths: [...connection.openPaths()] });
    else run("disable");
  });
  return {
    state, rename, references, notice, report: host.report,
    action(action: LspAction) { run("paths", { openPaths: [...connection.openPaths()] }); run("action", { action }); },
    select(index: number) { run("select", { index }); host.focus(); },
    geometry(rect: { x: number; y: number; width: number; height: number }, window: { width: number; height: number }, scale: number) {
      run("geometry", { ...rect, windowWidth: window.width, windowHeight: window.height, scale });
    },
    wheel(delta: number, precise: boolean) { run("wheel", { delta, precise }); },
    configureHover(config:Pick<AppConfig,"fontFamily"|"fontWeight"|"themeName">,viewport?:{x:number;y:number;width:number;height:number}){run("hover-configure",{fontFamily:config.fontFamily,fontWeight:config.fontWeight,theme:config.themeName,viewport:viewport?[viewport.x,viewport.y,viewport.width,viewport.height]:undefined});},
    hoverCaptures(x?:number,y?:number){const hover=state()?.hover;if(!hover)return false;if(hover.dragging)return true;if(x===undefined||y===undefined)return pointerInsideHover;const[bX,bY,width,height]=hover.rect;return x>=bX!&&x<bX!+width!&&y>=bY!&&y<bY!+height!;},
    hoverInput(phase:"down"|"move"|"up"|"wheel",x?:number,y?:number,delta?:number){
      clearTimeout(pointerTimer);pointerTick.cancel();latestPointer=undefined;
      const hover=state()?.hover,point={x:x??lastHoverPoint?.x??(hover?hover.rect[0]!+hover.rect[2]!/2:0),y:y??lastHoverPoint?.y??(hover?hover.rect[1]!+hover.rect[3]!/2:0)};
      lastHoverPoint=point;pointerInsideHover=!!hover&&point.x>=hover.rect[0]!&&point.x<hover.rect[0]!+hover.rect[2]!&&point.y>=hover.rect[1]!&&point.y<hover.rect[1]!+hover.rect[3]!;
      if(phase==="move"){
        latestHoverPointer=point;hoverTick.request(()=>{const point=latestHoverPointer;latestHoverPointer=undefined;if(point)run("hover-input",{phase,...point});});
      }else{hoverTick.cancel();latestHoverPointer=undefined;run("hover-input",{phase,...point,delta});}
    },
    retainPointer() { clearTimeout(pointerTimer); pointerTick.cancel(); latestPointer=undefined;pointerInsideHover=true;lastHoverPoint=undefined; },
    pointer(x?: number, y?: number) {
      clearTimeout(pointerTimer);
      pointerInsideHover=false;
      lastHoverPoint=undefined;
      if (x === undefined) {
        pointerTick.cancel(); latestPointer=undefined;
        pointerTimer = setTimeout(() => run("pointer", { x: null, y: null }), 180);
      } else {
        latestPointer={x,y:y??0};
        pointerTick.request(()=>{
          const point=latestPointer;latestPointer=undefined;
          if(disposed||!point)return;
          pointerAt=performance.now();run("pointer",point);
          // Start the hover probe after the actual native pointer update, so
          // even a delayed event loop preserves the original400ms threshold.
          clearTimeout(pointerTimer);
          pointerTimer=setTimeout(()=>{if(!disposed)editor.requestFrame();},420);
        },pointerAt);
      }
    },
    submitRename(name: string) { setRename(undefined); run("action", { action: "rename-submit", name }); host.focus(); },
    dismiss() { setRename(undefined); setReferences(undefined); run("action", { action: "dismiss" }); host.focus(); },
    openReference(row: ReferenceRow) {
      const root = references()?.root || connection.root;
      const path = row.path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(row.path) ? row.path : `${root.replace(/[\\/]$/, "")}/${row.path}`;
      setReferences(undefined); Promise.resolve(connection.openLocation(path, Math.max(0, row.line - 1), Math.max(0, row.column))).catch(host.report);
    },
    dispose() {
      unsubscribeStatus(); unsubscribeEditor(); unsubscribeDaemon();
      clearTimeout(pointerTimer); pointerTick.dispose();hoverTick.dispose(); clearTimeout(noticeTimer);
      if (connection.client.state.phase === "connected") { try { connection.client.send("Editor", { CloseLspBuffer: { surface_id: surface } }, connection.root); } catch {} }
      // The model may have released this editor before its Solid cleanup.
      try { editor.call("lsp.disable"); } catch {}
      disposed = true;
    },
  };
}
export type EditorLspController = ReturnType<typeof createEditorLsp>;
