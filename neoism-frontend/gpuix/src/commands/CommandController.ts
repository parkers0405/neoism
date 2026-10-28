import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { coreCall } from "../native-core";
import { lineCommandTarget } from "./chrome-keyboard";
export interface CommandTarget { action: string; argument?: string }
export interface CommandEntry { title: string; shortcut: string; action: string; service: string; glyph: string }
export interface CommandRow extends Omit<CommandEntry, "action"> { index: number; score: number; action: string | null; displayTitle: string; exCommand: string | null }
type Rect = [number, number, number, number];
export interface CommandFrame {
  rect: Rect; query: string; inputRect: Rect; listRect: Rect; selected: number; selectedY: number; top: number; offset: number;
  rowHeight: number; fontSize: number; inputFontSize: number; shortcutFontSize: number; stroke: number; radius: number;
  rows: CommandRow[]; target: CommandTarget | null; animating: boolean; visibleRows: number; cdQuery: boolean;
}
export interface CommandOptions {
  surface?: "file" | "editor" | "terminal" | "agent" | "markdown" | "notebook" | "epub";
  supportedActions?: readonly string[];
  scale?: number; width?: number; height?: number; query?: string; adaptiveTheme?: boolean;
  capabilities?: Partial<Record<"backgrounds" | "lsp_actions" | "notebook_kernel" | "draw_notes" | "leave_workspace" | "settings_editor" | "neoworld_page" | "minimap", boolean>>;
}
export class CommandController {
  readonly id: number;
  frame: CommandFrame;
  private listeners = new Set<(frame: CommandFrame) => void>();
  private animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  private disposed = false;
  private viewport: { width: number; height: number };
  private currentSurface:CommandOptions["surface"];
  constructor(options: CommandOptions = {}) {
    const started = performance.now();
    const result = coreCall<{ id: number; frame: CommandFrame }>("commands.new", options as Record<string, unknown>);
    this.id = result.id; this.frame = result.frame;
    this.viewport = { width: options.width || 1440, height: options.height || 900 };
    this.currentSurface=options.surface;
    this.animate(started);
  }
  static closeTarget(query:string){return coreCall<CommandTarget|null>("commands.close-target",{query})||undefined;}
  static catalog() { return coreCall<CommandEntry[]>("commands.catalog"); }
  subscribe = (listener: (frame: CommandFrame) => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private animate(started: number) { if (this.frame.animating) this.animation.request(() => this.call("frame"), started); }
  private call(op: string, args: Record<string, unknown> = {}) {
    if (this.disposed) return this.frame;
    const started = performance.now(); this.animation.cancel();
    this.frame = coreCall(`commands.${op}`, { id: this.id, ...args });
    for (const listener of this.listeners) listener(this.frame);
    this.animate(started);
    return this.frame;
  }
  query(query: string) { return this.call("query", { query }); }
  move(delta: number) { return this.call("move", { delta }); }
  select(index: number) { return this.call("select", { index }); }
  scroll(delta: number) { return this.call("scroll", { delta }); }
  layout(width: number, height: number) {
    if (width === this.viewport.width && height === this.viewport.height) return this.frame;
    this.viewport = { width, height }; return this.call("layout", this.viewport);
  }
  surface(surface: CommandOptions["surface"]) { this.currentSurface=surface;return this.call("surface", { surface }); }
  target() { return (["file","editor","markdown","notebook","epub"].includes(this.currentSurface||"") ? lineCommandTarget(this.frame.query) : undefined) || this.frame.target || undefined; }
  dispose() { if (!this.disposed) { this.disposed = true; this.animation.dispose(); this.listeners.clear(); coreCall("commands.dispose", { id: this.id }); } }
}
