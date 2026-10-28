import { createEffect, createSignal, For, onCleanup, onMount, untrack } from "solid-js";
import { useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { WorkspaceModel } from "../model";
import { coreCall } from "../native-core";
import { writeClipboard } from "../clipboard";
import { PaintLayer, type PaintCommand } from "../ui/PaintLayer";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusNativeElement } from "../native-focus";
import type {AgentFamilyRow} from "./family";
import { wheelCellPixels } from "../ui/wheel";
import {localPresenceIdentity} from "./local-presence";
import {handleApplicationShortcut} from "../commands/application-shortcuts";

interface TimelineFrame { commands: PaintCommand[]; animating: boolean; copy?: string | null; link?: string | null;navigate?:string;send?:unknown[];header?:Array<{direction:string;target:string;rects:number[][]}> }
interface Engine { id: number; dirty: Set<() => void>; dispose: () => void }

/** One original agent timeline engine per conversation (the shared Rust
 * `NeoismAgentPane`: cached layout, scroll-by-offset, stable-prefix
 * streaming, selection). It outlives tab switches like the desktop pane and
 * consumes the daemon's raw Agent events through the shared dispatcher. */
const engines = new WeakMap<WorkspaceModel, Map<string, Engine>>();
function engineFor(model: WorkspaceModel, sessionId: string, config: AppConfig,rootSessionId=sessionId): Engine {
  let byModel = engines.get(model);
  if (!byModel) { byModel = new Map(); engines.set(model, byModel); }
  const existing = byModel.get(rootSessionId); if (existing) return existing;
  const { id } = coreCall<{ id: number }>("agenttimeline.new", { sessionId,rootSessionId, theme: config.themeName, scale: config.scale,
    fontFamily: config.fontFamily, markdownFontFamily: config.markdownFontFamily, fontWeight: config.fontWeight,localName:localPresenceIdentity(config).displayName });
  const family=model.agentFamily(sessionId);
  coreCall("agenttimeline.family",{id,rootSessionId,sessionId,roster:family.roster,rootTitle:model.state.threads.find(thread=>thread.session_id===rootSessionId)?.title||rootSessionId});
  const dirty = new Set<() => void>();
  const event = (message: unknown) => {
    try { coreCall("agenttimeline.event", { id, message }); } catch (error) { model.report(error); return; }
    for (const listener of dirty) listener();
  };
  // Seed what the model already holds; later daemon events stream in live.
  for(const selected of new Set([rootSessionId,sessionId,...family.roster.map(row=>row.id)])){
    const history=model.state.history[selected];if(history?.length)event({HistoryChunk:{session_id:selected,messages:history,next_cursor:null}});
    const streaming=model.state.streaming[selected];if(streaming&&streaming!=="Idle")event({StreamingState:{session_id:selected,state:streaming,label:null}});
  }
  const unsubscribe = model.subscribeAgentEvents(event);
  const engine: Engine = { id, dirty, dispose: () => { unsubscribe(); byModel!.delete(rootSessionId); coreCall("agenttimeline.dispose", { id }); } };
  byModel.set(rootSessionId, engine);
  return engine;
}

export function AgentTimelineView(props: { sessionId: string; model: WorkspaceModel; config: AppConfig; report: (error: unknown) => void; openFile: (path: string) => void;
  /** Composer top in pane coordinates: layout ends here, paint continues under the floating island (desktop `render_chat`). */
  contentBottom?: number;rootSessionId?:string;familyRoster?:AgentFamilyRow[];onNavigate?:(sessionId:string)=>void }) {
  const config = props.config, s = config.scale, renderer = useGpuixRequired();
  const localName=localPresenceIdentity(config).displayName;
  const engine = untrack(() => engineFor(props.model, props.sessionId, config,props.rootSessionId||props.sessionId));
  const [commands, setCommands] = createSignal<readonly PaintCommand[]>([]);
  const [header,setHeader]=createSignal<NonNullable<TimelineFrame["header"]>>([]);
  let bounds = { x: 0, y: 0, width: 0, height: 0 }, host: HostElement | undefined, disposed = false, startedAt = performance.now();
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const apply = (next: TimelineFrame) => {
    setCommands(next.commands);setHeader(next.header||[]);
    for(const message of next.send||[])props.model.sendAgent(message as never);
    if(next.navigate)props.onNavigate?.(next.navigate);
    if (next.copy) void writeClipboard(next.copy).catch(props.report);
    if (next.link) {
      const target = next.link;
      if (/^(https?|mailto|tel):/i.test(target)) { const child = Bun.spawn([process.platform === "darwin" ? "open" : "xdg-open", target], { stderr: "ignore", stdout: "ignore" }); void child.exited; }
      else if (!target.startsWith("#")) props.openFile(target.split("#")[0]);
    }
    if (next.animating) schedule();
  };
  const call = (op: string, args: Record<string, unknown> = {}) => {
    startedAt = performance.now(); animation.cancel();
    try { apply(coreCall<TimelineFrame>(`agenttimeline.${op}`, { id: engine.id,...(op==="layout"?{localName}:{}), ...args })); } catch (error) { props.report(error); }
  };
  // One paint per display frame however many events or animations arrive.
  const schedule = () => { if (!disposed) animation.request(() => { if (!disposed && bounds.width > 0) call("frame"); }, startedAt); };
  engine.dirty.add(schedule);
  onCleanup(() => { disposed = true; engine.dirty.delete(schedule); animation.dispose(); });
  onMount(() => schedule());
  let familySignature="";
  createEffect(()=>{const family={rootSessionId:props.rootSessionId||props.sessionId,sessionId:props.sessionId,roster:props.familyRoster||[],rootTitle:props.model.state.threads.find(thread=>thread.session_id===(props.rootSessionId||props.sessionId))?.title||props.rootSessionId||props.sessionId};const signature=JSON.stringify(family);if(signature!==familySignature){familySignature=signature;untrack(()=>call("family",family));}});
  createEffect(() => { const bottom = props.contentBottom; if (bounds.width > 0) untrack(() => call("layout", { width: bounds.width, height: bounds.height, contentBottom: bottom ?? bounds.height })); });
  const local = (event: { x?: number; y?: number }) => ({ x: (event.x || 0) - bounds.x, y: (event.y || 0) - bounds.y });
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return;
    const key = event.key || "", ctrl = !!event.modifiers?.ctrl, modified = ctrl || !!event.modifiers?.alt || !!event.modifiers?.cmd;
    const handled = () => { event.preventDefault(); event.stopPropagation(); };
    if ((ctrl || event.modifiers?.cmd) && key.toLowerCase() === "c") {
      const text = coreCall<{ text: string | null }>("agenttimeline.copy", { id: engine.id }).text;
      if (text) { handled(); void writeClipboard(text).catch(props.report); }
    } else if (ctrl && ["d", "u"].includes(key)) { handled(); call("half", { older: key === "u" }); }
    else if (["pageup", "pagedown"].includes(key)) { handled(); call("scroll", { deltaY: (key === "pageup" ? 1 : -1) * bounds.height * 0.9, precise: true }); }
    else if (key === "home" || key === "end") { handled(); call("scroll", { deltaY: key === "home" ? 1e7 : -1e7, precise: true }); }
    else if (config.vim && !modified && ["j", "k"].includes(key)) { handled(); call("scroll", { deltaY: (key === "k" ? 1 : -1) * 19 * s, precise: true }); }
  };
  return <div ref={host} testId="conversation" tabIndex={0} aria-label="Conversation history" onKeyDown={keyDown}
    onLayout={(event) => {
      const next = { x: event.x || 0, y: event.y || 0, width: event.width || 0, height: event.height || 0 };
      const resized = next.width !== bounds.width || next.height !== bounds.height; bounds = next;
      if (resized && next.width > 0 && next.height > 0) call("layout", { width: next.width, height: next.height, contentBottom: props.contentBottom ?? next.height, scale: s, theme: config.themeName });
    }}
    onScroll={(event) => call("scroll", { deltaX: event.deltaX || 0, deltaY: event.deltaY || 0, precise: !!event.precise, lineHeight: wheelCellPixels(event, config.fontSize) })}
    onMouseDown={(event) => { if (host) focusNativeElement(renderer, host.id); if (event.button === 0) call("pointer", { kind: "down", ...local(event) }); }}
    onMouseMove={(event) => { try { coreCall("agenttimeline.pointer", { id: engine.id, kind: "move", deferFrame: true, ...local(event) }); } catch (error) { props.report(error); } schedule(); }}
    onMouseUp={(event) => call("pointer", { kind: "up", ...local(event) })}
    onMouseLeave={() => call("pointer", { kind: "leave" })}
    style={{ flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative", overflow: "hidden", userSelect: "none", cursor: "text" }}>
    <PaintLayer commands={commands()} config={config} />
    <For each={header()}>{link=><For each={link.rects}>{rect=><div testId={`agent-family-navigation:${link.direction}`} aria-label={link.direction==="next"?"Next conversation":"Previous conversation"} style={{position:"absolute",left:rect[0],top:rect[1],width:rect[2],height:rect[3],pointerEvents:"none"}}/>}</For>}</For>
  </div>;
}
