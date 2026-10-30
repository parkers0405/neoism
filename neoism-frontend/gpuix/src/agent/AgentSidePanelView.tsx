import { createEffect, createSignal, onCleanup, untrack } from "solid-js";
import { useGpuixRequired, type HostElement,type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { WorkspaceModel } from "../model";
import { coreCall } from "../native-core";
import { PaintLayer, type PaintCommand } from "../ui/PaintLayer";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusNativeElement,invalidateNativeFocus } from "../native-focus";
import {paintedChromeBounds,type ChromeBounds} from "../ui/chrome-target-bounds";
import {CanvasImages,RasterSource} from "../ui/canvas";

interface PanelFrame { commands: PaintCommand[]; animating: boolean; send: unknown[];cursor?:number[]|null;focused?:boolean;selected?:string; result?: { action?: string; anchor?: number[] | null; projects?: string[]; root?: boolean; open?: { id: string; title: string; directory?: string | null;familyRootId?:string|null;pointer?:boolean } } | null }

/** The original desktop conversations sidebar (shared `render_side_panel_with_icons`
 * over a catalog pane): project groups, running / sub-agent / background
 * indicators, header actions. Fed raw daemon Agent events; its own paging
 * requests go back out through the shared outbound mapping. */
/** One conversations panel per workspace, kept across workspace switches like
 * the desktop's `workspace_conversations_panes`: switching back repaints its
 * last frame at once and keeps its loaded pages instead of reloading. It stays
 * subscribed while hidden so its catalog is current when shown again. */
interface CachedPanel { id: number; workspace: string; style: string; directory: string; commands: readonly PaintCommand[]; viewport:{width:number;height:number};images:Map<object,CanvasImages>; dirty: Set<() => void>; release: () => void }
const panels = new WeakMap<WorkspaceModel, Map<string, CachedPanel>>();
function panelFor(model: WorkspaceModel, workspace: string, directory: string, config: AppConfig, report: (error: unknown) => void): CachedPanel {
  let byModel = panels.get(model);
  if (!byModel) { byModel = new Map(); panels.set(model, byModel); }
  // The native panel paints at a fixed scale/theme/font, so the cache key
  // includes them. A zoom builds a new one while the outgoing generation
  // still shows the old; that one is released once nothing displays it.
  const style = JSON.stringify([config.scale, config.themeName, config.fontFamily, config.fontWeight]);
  const key = `${workspace}\0${style}`;
  const existing = byModel.get(key);
  if (existing) return existing;
  const { id } = coreCall<{ id: number }>("agentsidepanel.new", { directory, theme: config.themeName, scale: config.scale, fontFamily: config.fontFamily, fontWeight: config.fontWeight });
  const unsubscribe = model.subscribeAgentEvents((message) => {
    try { coreCall("agentsidepanel.event", { id, message }); } catch (error) { report(error); return; }
    for (const listener of panel.dirty) listener();
  });
  const panel: CachedPanel = { id, workspace, style, directory, commands: [],viewport:{width:0,height:0},images:new Map(), dirty: new Set(),
    release: () => { unsubscribe();for(const images of panel.images.values())images.dispose();panel.images.clear(); byModel!.delete(key); coreCall("agentsidepanel.dispose", { id }); } };
  byModel.set(key, panel);
  for (const other of byModel.values()) if (other.workspace === workspace && other !== panel) retireWhenHidden(other);
  return panel;
}
function retireWhenHidden(panel: CachedPanel) {
  const check = () => { if (panel.dirty.size) setTimeout(check, 1000); else panel.release(); };
  setTimeout(check, 1000);
}

export function AgentSidePanelView(props: { model: WorkspaceModel; config: AppConfig; directory?: string | null; viewedSession?: string;
  /** Workspace identity the panel is cached under. */
  cacheKey?: string;
  onHost?:(host:HostElement|undefined)=>void;onCursor?:(cursor:ChromeBounds|undefined,host:HostElement,bounds:ChromeBounds)=>void;
  onFocus?:()=>void;onActivate?:()=>void;onCustomKey?:(event:KeyEvent)=>boolean;
  onAction: (action: string, detail: { anchor?: number[]; projects: string[]; root: boolean }) => void; report: (error: unknown) => void }) {
  const config = props.config, model = props.model, renderer = useGpuixRequired();
  const panel = untrack(() => props.cacheKey
    ? panelFor(model, props.cacheKey, props.directory || "", config, props.report)
    : undefined);
  const id = panel?.id ?? untrack(() => coreCall<{ id: number }>("agentsidepanel.new", { directory: props.directory || "", theme: config.themeName, scale: config.scale, fontFamily: config.fontFamily, fontWeight: config.fontWeight }).id);
  const [commands, setCommands] = createSignal<readonly PaintCommand[]>(panel?.commands ?? []);
  const [selected,setSelected]=createSignal("");
  const selectionDescription=()=>selected()==="new-chat"?"New chat":selected()==="search"?"Search":selected()==="projects"?"Project directory":selected().startsWith("row:")?`Conversation: ${model.state.threads.find(thread=>thread.session_id===selected().slice(4))?.title||model.agentFamily(selected().slice(4)).roster.find(row=>row.id===selected().slice(4))?.title||"Selected conversation"}`:"Conversation browser";
  let bounds = { x: 0, y: 0, width:panel?.viewport.width||0,height:panel?.viewport.height||0 }, host: HostElement | undefined, disposed = false, startedAt = performance.now();
  let actualFocused=false;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const apply = (next: PanelFrame) => {
    if(panel){let images=panel.images.get(renderer);if(!images){images=new CanvasImages(renderer);panel.images.set(renderer,images);}for(const command of next.commands){if(command.kind==="text"&&command.raster)images.use(RasterSource.painter,command.raster);else if(command.kind==="image")images.use(RasterSource.paintImage,command.raster);}images.commit();}
    setCommands(next.commands); if (panel) panel.commands = next.commands;
    setSelected(next.selected||"");
    if(host){const painted=paintedChromeBounds(renderer,host.id);if(painted)bounds={...bounds,x:painted.x,y:painted.y};const rect=next.cursor;props.onCursor?.(rect?{x:bounds.x+rect[0]!,y:bounds.y+rect[1]!,width:rect[2]!,height:rect[3]!}:undefined,host,bounds);}
    for (const message of next.send || []) model.sendAgent(message as never);
    const result = next.result;
    if (result?.open) {
      const known = model.state.threads.find((thread) => thread.session_id === result.open!.id);
      model.openThread(known || { session_id: result.open.id, title: result.open.title, directory: result.open.directory ?? props.directory ?? "" } as never,false,result.open.familyRootId||undefined,!!result.open.pointer);
      props.onActivate?.();
    } else if (result?.action) {
      // Panel-local anchor → window coordinates for the dropdown overlay.
      const anchor = result.anchor ? [result.anchor[0] + bounds.x, result.anchor[1] + bounds.y, result.anchor[2], result.anchor[3]] : undefined;
      props.onAction(result.action, { anchor, projects: result.projects || [], root: !!result.root });
    }
    if (next.animating) schedule();
  };
  const call = (op: string, args: Record<string, unknown> = {}) => {
    startedAt = performance.now(); animation.cancel();
    try { apply(coreCall<PanelFrame>(`agentsidepanel.${op}`, { id, ...args })); } catch (error) { props.report(error); }
  };
  const schedule = () => { if (!disposed) animation.request(() => { if (!disposed && bounds.width > 0) call("frame", { viewedSession: props.viewedSession ?? null }); }, startedAt); };
  const unsubscribe = panel
    ? (panel.dirty.add(schedule), () => { panel.dirty.delete(schedule); })
    : model.subscribeAgentEvents((message) => {
      try { coreCall("agentsidepanel.event", { id, message }); } catch (error) { props.report(error); return; }
      schedule();
    });
  // The catalog pane requests its own pages (desktop semantics: a null
  // directory is the all-workspace catalog); replies return through the
  // shared dispatcher verbatim.
  createEffect(() => { props.viewedSession; schedule(); });
  // Follow the workspace directory: unknown at mount, or changed in place.
  let scoped = panel?.directory ?? (props.directory || "");
  createEffect(() => { const directory = props.directory || ""; if (directory !== scoped) { scoped = directory; if (panel) panel.directory = directory; call("scope", { directory });if(actualFocused)call("focus",{focused:true}); } });
  onCleanup(() => { disposed = true;props.onHost?.(undefined); unsubscribe(); animation.dispose(); if (!panel) coreCall("agentsidepanel.dispose", { id });else if(actualFocused)coreCall("agentsidepanel.focus",{id,focused:false}); });
  const local = (event: { x?: number; y?: number }) => ({ x: (event.x || 0) - bounds.x, y: (event.y || 0) - bounds.y });
  return <div ref={value=>{host=value;props.onHost?.(value);}} tabIndex={0} testId="conversation-list" aria-label="Conversations" aria-description={selectionDescription()}
    onFocus={()=>{invalidateNativeFocus(renderer);actualFocused=true;call("focus",{focused:true});props.onFocus?.();}}
    onBlur={()=>{invalidateNativeFocus(renderer);actualFocused=false;call("focus",{focused:false});}}
    onKeyDown={event=>{if(props.onCustomKey?.(event))return;if(event.modifiers?.alt||event.modifiers?.cmd||event.modifiers?.shift)return;const key=event.key?.toLowerCase()||"",ctrl=!!event.modifiers?.ctrl;if(!["up","down","left","right","enter","escape"].includes(key)&&!(ctrl&&["d","u","f"].includes(key)))return;event.preventDefault();event.stopPropagation();call("key",{key,ctrl});}}
    onLayout={(event) => {
      const next = { x: event.x || 0, y: event.y || 0, width: event.width || 0, height: event.height || 0 };
      const resized = next.width !== bounds.width || next.height !== bounds.height; bounds = next;
      if(panel&&next.width>0&&next.height>0)panel.viewport={width:next.width,height:next.height};
      if (resized && next.width > 0 && next.height > 0) call("frame", { width: next.width, height: next.height, theme: config.themeName, viewedSession: props.viewedSession ?? null });
    }}
    onScroll={(event) => call("scroll", { deltaY: event.deltaY || 0, precise: !!event.precise })}
    onMouseDown={(event) => { if (host) focusNativeElement(renderer, host.id); if (event.button === 0) call("pointer", { kind: "down", ...local(event) }); }}
    onMouseMove={(event) => { try { coreCall("agentsidepanel.pointer", { id, kind: "move", ...local(event) }); } catch {} schedule(); }}
    onMouseLeave={() => call("pointer", { kind: "leave" })}
    style={{ flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative", overflow: "hidden", userSelect: "none", cursor: "pointer" }}>
    <PaintLayer commands={commands()} config={config} />
  </div>;
}
