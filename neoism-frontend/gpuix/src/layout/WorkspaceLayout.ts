import { coreCall } from "../native-core";

export type SplitAxis = "horizontal" | "vertical";
export type FocusDirection = "left" | "right" | "up" | "down" | "next" | "previous" | "first" | "last";
export type LayoutRect = [number, number, number, number];
export interface LayoutSurface { id: string; title: string; kind: "file" | "agent" | "terminal" | "page" | "welcome" | "git" }
export interface LayoutTab { leaf: number; externalId: number | null; surfaceId?: string; title: string; kind: LayoutSurface["kind"] }
export interface LayoutPane {
  leaf: number; externalId: number | null; surfaceId?: string; rect: LayoutRect; focused: boolean; path: number[]; tabs: LayoutTab[];
}
export interface LayoutDivider { path: number[]; gap: number; axis: SplitAxis; rect: LayoutRect; ratio: number; splitRect: LayoutRect; available: number }
export interface LayoutFrame {
  focusedLeaf: number | null; focusedExternalId: number | null; focusedSurfaceId?: string;
  panes: LayoutPane[]; dividers: LayoutDivider[]; leaves: LayoutTab[]; tree: unknown;
}
export interface LayoutSnapshot { version: 1; tree: unknown; routes: Record<number, LayoutSurface> }
export interface DaemonLayoutLeaf { pane_external_id: number; surface_id: string; session_id: string; path?: string | null; route_id?: number | null }
export interface DropPreview { target: number | null; placement: "left" | "right" | "top" | "bottom" | "center" | null; rect: LayoutRect | null }

/** One workspace's renderer-independent tree. Rust owns all layout decisions. */
export class WorkspaceLayout {
  readonly id: number;
  frame: LayoutFrame;
  private routes = new Map<number, LayoutSurface>();
  private nextExternal = 1;
  private listeners = new Set<(frame: LayoutFrame) => void>();
  private disposed = false;
  private viewport = { width: 0, height: 0, scale: 1 };
  constructor(seed?: LayoutSnapshot) {
    if (seed && seed.version !== 1) throw new Error("Unsupported saved pane layout");
    if (seed) for (const [key, surface] of Object.entries(seed.routes)) {
      const id = Number(key);
      if (!Number.isSafeInteger(id) || id < 1 || !surface || typeof surface.id !== "string" || typeof surface.title !== "string" || !["file", "agent", "terminal", "page", "welcome", "git"].includes(surface.kind)) throw new Error("Invalid saved pane route");
      this.routes.set(id, { ...surface, title:surface.kind==="welcome"?"Home":surface.title }); this.nextExternal = Math.max(this.nextExternal, id + 1);
    }
    const result = coreCall<{ id: number; frame: LayoutFrame }>("layout.new", { tree: seed?.tree });
    this.id = result.id; this.frame = this.decorate(result.frame);
  }
  subscribe = (listener: (frame: LayoutFrame) => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private decorate(frame: LayoutFrame): LayoutFrame {
    const tab = (value: LayoutTab): LayoutTab => {
      const surface = value.externalId == null ? undefined : this.routes.get(value.externalId);
      return { ...value, surfaceId: surface?.id, title: surface?.title || value.title || "Untitled", kind: surface?.kind || value.kind };
    };
    return { ...frame, focusedSurfaceId: frame.focusedExternalId == null ? undefined : this.routes.get(frame.focusedExternalId)?.id,
      panes: frame.panes.map((p) => ({ ...p, surfaceId: p.externalId == null ? undefined : this.routes.get(p.externalId)?.id, tabs: p.tabs.map(tab) })), leaves: frame.leaves.map(tab) };
  }
  private call(op: string, args: Record<string, unknown> = {}) {
    if (this.disposed) throw new Error("Pane layout is disposed");
    this.frame = this.decorate(coreCall<LayoutFrame>(`layout.${op}`, { id: this.id, ...args }));
    for (const listener of this.listeners) listener(this.frame);
    return this.frame;
  }
  private register(surface: LayoutSurface) {
    const existing = [...this.routes].find(([, entry]) => entry.id === surface.id)?.[0];
    const id = existing ?? this.nextExternal++;
    const title=surface.kind==="welcome"?"Home":surface.title;
    this.routes.set(id, { ...surface, title });
    return { externalId: id, title, kind: surface.kind };
  }
  open(surface: LayoutSurface) { return this.call("open", this.register(surface)); }
  split(axis: SplitAxis, surface: LayoutSurface, before = false) { return this.call("split", { ...this.register(surface), axis, before }); }
  focus(leaf: number) { return this.frame.focusedLeaf === leaf ? this.frame : this.call("focus", { leaf }); }
  focusSurface(surfaceId: string) {
    const leaf = this.frame.leaves.find((l) => l.surfaceId === surfaceId)?.leaf;
    return leaf === undefined ? this.frame : this.focus(leaf);
  }
  firstTab(surfaceId:string) {
    const leaf=this.frame.leaves.find(leaf=>leaf.surfaceId===surfaceId)?.leaf;
    return leaf===undefined?this.frame:this.call("first-tab",{leaf});
  }
  reorder(leaf:number,target:number,before=true) { return this.call("reorder",{leaf,target,before}); }
  focusDirection(direction: FocusDirection) { return this.call("focus-direction", { direction }); }
  close(leaf: number) { return this.call("close", { leaf }); }
  closeSurface(surfaceId: string) {
    for (const leaf of this.frame.leaves.filter((l) => l.surfaceId === surfaceId)) this.close(leaf.leaf);
    return this.frame;
  }
  rename(surface: LayoutSurface) {
    this.register(surface);
    for (const leaf of this.frame.leaves.filter((l) => l.surfaceId === surface.id)) this.call("rename", { leaf: leaf.leaf, title: surface.title });
    return this.frame;
  }
  layout(width: number, height: number, scale = 1) {
    if (this.viewport.width === width && this.viewport.height === height && this.viewport.scale === scale) return this.frame;
    this.viewport = { width, height, scale };
    return this.call("size", this.viewport);
  }
  resize(path: number[], gap: number, ratio: number) { return this.call("resize", { path, gap, ratio }); }
  nudge(axis: SplitAxis, delta: number) { return this.call("nudge", { axis, delta }); }
  dropPreview(x: number, y: number): DropPreview { return coreCall("layout.drop-preview", { id: this.id, x, y }); }
  drop(leaf: number, x: number, y: number) { return this.call("drop", { leaf, x, y }); }
  snapshot(): LayoutSnapshot {
    const used = new Set(this.frame.leaves.map((l) => l.externalId));
    return { version: 1, tree: structuredClone(this.frame.tree), routes: Object.fromEntries([...this.routes].filter(([id]) => used.has(id)).map(([id, value]) => [id, { ...value }])) };
  }
  /** Returns false for an empty/unresolvable daemon layout without changing it. */
  restoreDaemon(snapshot: unknown, resolve: (externalId: number, path: string | undefined, leaf: DaemonLayoutLeaf) => LayoutSurface | undefined) {
    const value = typeof snapshot === "string" ? JSON.parse(snapshot) : snapshot;
    const leaves: DaemonLayoutLeaf[] = [];
    const walk = (node: any) => {
      if (node?.kind === "leaf" && Number.isSafeInteger(node.pane_external_id)) leaves.push(node);
      else if (Array.isArray(node?.children)) node.children.forEach(walk);
    };
    walk(value?.root);
    const resolved = leaves.map((leaf) => [leaf.pane_external_id, resolve(leaf.pane_external_id, leaf.path || undefined, leaf)] as const).filter((entry): entry is readonly [number, LayoutSurface] => !!entry[1]);
    if (!resolved.length) return false;
    const next = coreCall<LayoutFrame>("layout.restore-daemon", { id: this.id, snapshot: value });
    // Do not replace route bindings until the canonical decoder accepts the tree.
    this.routes = new Map(resolved); this.nextExternal = Math.max(...resolved.map(([id]) => id), 0) + 1;
    this.frame = this.decorate(next);
    for (const leaf of this.frame.leaves.filter((l) => !l.surfaceId)) this.close(leaf.leaf);
    this.call("size", this.viewport);
    return true;
  }
  dispose() { if (!this.disposed) { coreCall("layout.dispose", { id: this.id }); this.disposed = true; this.listeners.clear(); } }
}
