import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { coreCall } from "../native-core";
import { isRecord, variant, type SearchClientMessage } from "../daemon/protocol";
import type { AppConfig } from "../config";
import type { PaintCommand } from "../ui/PaintLayer";
import { sharePaintCommands } from "../editor/paint-command";
export type FinderMode = "files" | "grep" | "git" | "buffer" | "replace" | "references" | "symbols";
export interface FinderTarget { path?: string; line?: number | null; column?: number | null; mode: FinderMode; query: string }
export interface FinderRow { index: number; path: string; line: number | null; column: number | null; text: string; marker: string; symbol: string | null; score: number }
type Rect = [number, number, number, number];
interface Request { id: number; message: SearchClientMessage }
export interface FinderFrame {
  commands: PaintCommand[]; cursorQuad: number[][]; cursorHold?: boolean;
  enabled: boolean; mode: FinderMode; query: string; rect: Rect; radius: number; stroke: number;
  inputRect: Rect; listRect: Rect; previewRect: Rect; rowHeight: number; inputFontSize: number; fontSize: number;
  previewFontSize: number; previewLineHeight: number; selected: number; selectedY: number; offset: number; top: number; visibleRows: number;
  rows: FinderRow[]; target: FinderTarget | null; badge: string; matchTotal: number; placeholder: string; empty: string;
  preview: boolean; previewPath: string | null; previewRows: Array<{ line: number; text: string; tokens: Array<{ token: string; text: string }>; y: number; selected: boolean }>;
  previewUnreadable: boolean; previewLoaded: boolean; animating: boolean; nextTickMs: number | null; requests: Request[]; loading: boolean; error?: string;
}
export interface FinderOptions {
  cwd: string; mode?: FinderMode; scale?: number; top?: number; width?: number; height?: number;
  lines?: readonly string[]; references?: ReadonlyArray<{ path: string; line: number; column: number; text: string }>;
  symbols?: ReadonlyArray<{ kind: string; name: string; line: number; column: number }>; loading?: boolean;
  fontFamily?: string; fontWeight?: number; iconFontFamily?: string; theme?: string;
  search: (message: SearchClientMessage, signal: AbortSignal) => Promise<unknown>;
  preview?: (path: string, signal: AbortSignal) => Promise<string>;
}
/** Async capabilities surround the same native Finder used by the desktop. */
export class FinderController {
  readonly id: number;
  frame: FinderFrame;
  private listeners = new Set<(frame: FinderFrame) => void>();
  private pending = new Map<number, AbortController>();
  private previewRequest?: AbortController;
  private previewing?: string;
  private generation = 0;
  private error?: string;
  private appearanceKey = "";
  private disposed = false;
  private animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  private viewport: { width: number; height: number };
  constructor(private options: FinderOptions) {
    const started = performance.now();
    const { search: _search, preview: _preview, ...args } = options;
    const result = coreCall<{ id: number; frame: FinderFrame }>("finder.new", args);
    this.id = result.id; this.frame = result.frame;
    this.viewport = { width: options.width || 1440, height: options.height || 900 };
    this.consume(started);
  }
  subscribe = (listener: (frame: FinderFrame) => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private call(op: string, args: Record<string, unknown> = {}) {
    if (this.disposed) return this.frame;
    const started = performance.now(); this.animation.cancel();
    const next = coreCall<FinderFrame>(`finder.${op}`, { id: this.id, ...args });
    this.frame = { ...next, commands: sharePaintCommands(this.frame.commands || [], next.commands || []), error: this.error };
    for (const listener of this.listeners) listener(this.frame);
    this.consume(started);
    return this.frame;
  }
  private consume(started: number) {
    if (this.disposed) return;
    const frame = this.frame, generation = this.generation;
    for (const request of frame.requests) {
      const controller = new AbortController(); this.pending.set(request.id, controller);
      void Promise.resolve().then(() => this.options.search(request.message, controller.signal)).then((reply) => {
        if (this.disposed || generation !== this.generation || controller.signal.aborted) return;
        const entry = variant(reply);
        if (!entry || !isRecord(entry[1])) throw new Error("Invalid search response");
        if (entry[0] === "SearchError" || entry[0] === "Error") throw new Error(typeof entry[1].message === "string" ? entry[1].message : "Search failed");
        this.pending.delete(request.id);
        this.call("reply", { requestId: request.id, message: { [entry[0]]: { ...entry[1], req_id: request.id } } });
      }).catch((error) => {
        if (this.disposed || generation !== this.generation || controller.signal.aborted) return;
        this.pending.delete(request.id);
        this.error = error instanceof Error ? error.message : "Search failed";
        this.call("reply", { requestId: request.id, message: { SearchError: { req_id: request.id, message: "Search failed" } } });
        this.frame = { ...this.frame, error: this.error, loading: this.pending.size > 0 };
        for (const listener of this.listeners) listener(this.frame);
      });
    }
    if (!frame.preview || !frame.previewPath) {
      this.previewRequest?.abort(); this.previewRequest = undefined; this.previewing = undefined;
    }
    if (frame.preview && frame.previewPath && !frame.previewLoaded && frame.previewPath !== this.previewing && this.options.preview) {
      this.previewRequest?.abort();
      const controller = new AbortController(), path = frame.previewPath;
      this.previewRequest = controller; this.previewing = path;
      void Promise.resolve().then(() => this.options.preview!(path, controller.signal)).then((text) => {
        if (!this.disposed && !controller.signal.aborted) { this.previewing = undefined; this.call("preview", { path, text }); }
      }).catch(() => { if (!this.disposed && !controller.signal.aborted) { this.previewing = undefined; this.call("preview", { path, text: null }); } });
    }
    const delay = Math.min(frame.animating ? ANIMATION_FRAME_MS : Infinity, frame.nextTickMs ?? Infinity);
    if (Number.isFinite(delay)) this.animation.request(() => this.call("frame"), started, delay);
  }
  query(query: string) {
    if (query === this.frame.query) return this.frame;
    this.error = undefined;
    this.generation++; for (const controller of this.pending.values()) controller.abort(); this.pending.clear();
    return this.call("query", { query });
  }
  cycle() {
    this.error = undefined;
    this.generation++; for (const controller of this.pending.values()) controller.abort(); this.pending.clear();
    return this.call("cycle");
  }
  move(delta: number) { return this.call("move", { delta }); }
  select(index: number) { return this.call("select", { index }); }
  scroll(delta: number, touch = false) { return this.call(touch ? "touch-scroll" : "scroll", { delta }); }
  layout(width: number, height: number) {
    if (this.viewport.width === width && this.viewport.height === height) return this.frame;
    this.viewport = { width, height }; return this.call("layout", this.viewport);
  }
  appearance(config: Pick<AppConfig,"fontFamily"|"fontWeight"|"iconFontFamily"|"themeName">) {
    const args={fontFamily:config.fontFamily,fontWeight:config.fontWeight,iconFontFamily:config.iconFontFamily,theme:config.themeName},key=JSON.stringify(args);
    if(key===this.appearanceKey)return this.frame;
    this.appearanceKey=key;return this.call("style",args);
  }
  symbols(symbols: FinderOptions["symbols"], loading = false) { return this.call("symbols", { symbols, loading }); }
  selectedTarget(): FinderTarget | undefined {
    const target = this.frame.target; if (!target) return;
    const buffer = ["buffer", "replace", "symbols"].includes(target.mode);
    const path = target.path?.replaceAll("\\", "/"), root = this.options.cwd.replaceAll("\\", "/").replace(/\/$/, "");
    const windows = /^[a-z]:/i.test(root), prefix = root + "/";
    const sameRoot = path && (windows ? path.toLowerCase().startsWith(prefix.toLowerCase()) : path.startsWith(prefix));
    return { ...target, path: buffer ? undefined : sameRoot ? path.slice(prefix.length) : path };
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true; this.animation.dispose(); this.generation++;
    for (const controller of this.pending.values()) controller.abort(); this.pending.clear(); this.previewRequest?.abort(); this.listeners.clear();
    coreCall("finder.dispose", { id: this.id });
  }
}
