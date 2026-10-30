import { dlopen, FFIType, ptr, CString, type Pointer } from "bun:ffi";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { shareEditorPaint, shareLspFrame, unpackEditorPaint } from "./editor/paint-frame";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "./animation/frame-scheduler";
import { profileStart, profileEnd } from "./profiling";

let binding: ReturnType<typeof open> | undefined;
function open() {
  const name = process.platform === "win32" ? "neoism_gpuix_core.dll" : process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : "libneoism_gpuix_core.so";
  const paths = [process.env.NEOISM_GPUIX_CORE, join(dirname(process.execPath), name), fileURLToPath(new URL(`../artifacts/${name}`, import.meta.url))].filter((path): path is string => !!path);
  const path = paths.find(existsSync);
  if (!path) throw new Error("Neoism's native engine is not built. Run npm run build:core in neoism-frontend/gpuix.");
  return dlopen(path, {
    neoism_core_call: { args: [FFIType.ptr, FFIType.u64], returns: FFIType.ptr },
    neoism_core_free: { args: [FFIType.ptr], returns: FFIType.void },
  });
}
export function coreCall<T = unknown>(op: string, args: Record<string, unknown> = {}): T {
  const started=profileStart();
  try {
  binding ??= open();
  const encodeStart=profileStart();
  const bytes = new TextEncoder().encode(JSON.stringify({ op, ...args }));
  profileEnd("core.encode",encodeStart,bytes.byteLength);
  const nativeStart=profileStart();
  const result = binding.symbols.neoism_core_call(ptr(bytes), bytes.byteLength) as Pointer;
  profileEnd(`native.${op}`,nativeStart);
  if (!result) throw new Error("Native engine returned no response");
  try {
    const decodeStart=profileStart(),json=new CString(result).toString();
    const value: { ok?: T; error?: string } = JSON.parse(json);
    profileEnd("core.decode",decodeStart,json.length);
    if (value.error) throw new Error(value.error);
    return value.ok as T;
  } finally { binding.symbols.neoism_core_free(result); }
  } finally { profileEnd(`core.${op}`,started); }
}
export interface EditorRun { text: string; x: number; width?: number; token: string; selected: boolean; severity?: string | null }
export interface EditorSnapshot { version: 1; text: string; buffer: unknown; [key: string]: unknown }
export interface EditorRowRaster { id:number; ptr:number; len:number; width:number; height:number; scale:number; x:number; y:number }
export interface EditorFrame {
  collaborationBound?:boolean;
  presence?:{cursor:import("./daemon/protocol").CrdtCursorPosition;selection:import("./daemon/protocol").CrdtSelectionRange|null;insert:boolean};
  remotePeers?:import("./daemon/protocol").CrdtPeerPresence[];
  peerCommands?:import("./ui/PaintLayer").PaintCommand[];
  packedRuns?: boolean;
  wrap?: boolean; vim?: boolean;
  lsp?: import("./editor/lsp").LspFrame;
  lines: Array<{ line: number; continuation: boolean; y: number; runs: EditorRun[]; raster?:EditorRowRaster|null }>;
  rowHeight: number; cellWidth: number; textX: number; gutterWidth: number;
  cursor: { x: number; y: number; line: number; col: number; shape: "Block" | "Beam"; visible: boolean };
  mode: "Normal" | "Insert" | "Visual"; dirty: boolean; revision: number;
  totalLines: number; pendingKeys: string;
  scrollY: number; contentHeight: number; animating: boolean; scrollAnimating?: boolean;
  hostAction?: "save" | "save-close" | "close"; searchHits: { x: number; y: number; width: number; selection?: boolean }[];
  extraCarets: { x: number; y: number; width: number; height: number }[];
  clipboard?: string | null; pendingEmpty: boolean;
  cursorQuad: number[][]; cursorHold: boolean;
  selection: { start: number[]; end: number[] } | null;
}
export class NativeEditor {
  readonly id: number;
  private historyOwner?:NativeEditor;
  private listeners = new Set<(frame: EditorFrame) => void>();
  private animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  private lastFrameStarted = performance.now();
  private pendingScroll = 0;
  private fullFrameRequested = false;
  private configuration = new Map<string,string>();
  subscribe(listener: (frame: EditorFrame) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  frame: EditorFrame;
  constructor(path: string, text: string, vim: boolean, viewOf?: NativeEditor) {
    this.historyOwner=viewOf;
    this.id = viewOf ? coreCall<{ id: number }>("editor.clone-view", { id: viewOf.id }).id
      : coreCall<{ id: number }>("editor.new", { path, text, vim, compact: true }).id;
    this.frame = unpackEditorPaint(coreCall<EditorFrame>("editor.frame", { id: this.id }));
  }
  getPresence(){return this.frame.presence;}
  call(op: string, args: Record<string, unknown> = {}):EditorFrame {
    if(op==="crdt.history"&&this.historyOwner)return this.historyOwner.call(op,args);
    const configurable=op==="layout"||op==="raster.configure";
    const signature=configurable?JSON.stringify(args):undefined;
    if(signature!==undefined&&this.configuration.get(op)===signature)return this.frame;
    const overlayOnly=["lsp.pointer","lsp.geometry","lsp.hover-configure","lsp.hover-input","lsp.wheel"].includes(op);
    if(!overlayOnly){this.animation.cancel(); this.fullFrameRequested = false; this.lastFrameStarted = performance.now();}
    const next = coreCall<EditorFrame | {lspOnly:true;lsp:EditorFrame["lsp"]} | {cursorOnly:true;cursorQuad:number[][];cursorHold:boolean;animating:boolean}>(`editor.${op}`, { id: this.id, ...args, ...(overlayOnly?{overlayOnly:true}:{}) });
    if("lspOnly" in next){
      // Hover does not mutate the document or tick its springs. In particular,
      // never cancel an already queued scroll/key frame for pointer traffic.
      this.frame={...this.frame,lsp:shareLspFrame(this.frame.lsp,next.lsp),clipboard:null,hostAction:undefined};
    } else if ("cursorOnly" in next) {
      // Retain the document display list and never replay one-shot effects
      // from the full frame that positioned this cursor.
      const lsp=this.frame.lsp;
      this.frame={...this.frame,cursorQuad:next.cursorQuad,cursorHold:next.cursorHold,animating:next.animating,clipboard:null,hostAction:undefined,
        lsp:lsp&&(lsp.requests.length||lsp.events.length)?{...lsp,requests:[],events:[]}:lsp};
    } else this.frame = shareEditorPaint(this.frame, unpackEditorPaint(next));
    if(signature!==undefined)this.configuration.set(op,signature);
    if(this.historyOwner&&this.frame.collaborationBound){
      const requests=coreCall<boolean[]>("editor.crdt.take-history",{id:this.id});
      for(const redo of requests)this.historyOwner.call("crdt.history",{redo});
    }
    const publishStart=profileStart();
    for (const listener of this.listeners) listener(this.frame);
    profileEnd("editor.publish",publishStart);
    if (this.pendingScroll) this.requestFrame();
    return this.frame;
  }
  /** Apply motion immediately, but paint/tick springs on the native display
   * frame. Input repeat events never cancel an already-requested frame. */
  queueKey(args: Record<string, unknown>) {
    const next = coreCall<EditorFrame | { deferred:true; cursor:Pick<EditorFrame["cursor"],"line"|"col">;mode:EditorFrame["mode"];pendingEmpty:boolean;pendingKeys:string;selection:EditorFrame["selection"] }>("editor.key", {id:this.id,...args,deferFrame:true});
    if ("deferred" in next) {
      // Refresh logical state and snapshot identity immediately, without
      // publishing the old paint geometry as a new rendered frame.
      this.frame={...this.frame,cursor:{...this.frame.cursor,...next.cursor},mode:next.mode,pendingEmpty:next.pendingEmpty,pendingKeys:next.pendingKeys,selection:next.selection};
      this.requestFrame();
    } else {
      // Vim operator motions can change the document or clipboard. Preserve
      // the normal synchronous publication/document synchronization path.
      this.animation.cancel();this.lastFrameStarted=performance.now();
      this.frame=shareEditorPaint(this.frame,unpackEditorPaint(next));
      for(const listener of this.listeners)listener(this.frame);
      if(this.pendingScroll)this.requestFrame();
    }
    return this.frame;
  }
  queueScroll(delta: number) {
    if (!Number.isFinite(delta) || !delta) return;
    this.pendingScroll += delta;
    this.requestFrame();
  }
  requestFrame(cursorOnly = false) {
    // Animation owners may reuse the document once scrolling settles. Any
    // deferred input, LSP probe, or other full-frame request takes precedence.
    this.fullFrameRequested ||= !cursorOnly;
    // Editor and LSP animations share one on-demand budget. Leave headroom
    // for high-refresh native presentation instead of imposing a 120Hz cap.
    this.animation.request(() => {
      const delta = this.pendingScroll; this.pendingScroll = 0;
      const full = this.fullFrameRequested; this.fullFrameRequested = false;
      this.call(delta ? "scroll" : !full && this.frame.scrollAnimating === false ? "cursor-frame" : "frame", delta ? { delta } : {});
    }, this.lastFrameStarted);
  }
  cancelFrame() { this.animation.cancel(); this.pendingScroll = 0; this.fullFrameRequested = false; }
  text() { return coreCall<{ text: string }>("editor.text", { id: this.id }).text; }
  snapshot() { return coreCall<EditorSnapshot>("editor.snapshot", { id: this.id }); }
  restore(snapshot: EditorSnapshot) { return this.call("restore", { snapshot }); }
  cloneView() { return new NativeEditor("", "", false, this); }
  syncFrom(source: NativeEditor) { return this.call("sync", { sourceID: source.id }); }
  dispose() { this.animation.dispose(); this.listeners.clear(); coreCall("dispose", { id: this.id }); }
}

export interface SearchHost {
  preview(pattern: string): void;
  commit(pattern: string): void;
  cancel(): void;
  focus(): void;
}
