import type {CanvasRaster} from "../ui/canvas";
import type {ImagePreview} from "../ui/AttachmentImage";
import { batch, createEffect, createSignal, For, Show, onCleanup, onMount, untrack } from "solid-js";
import { useGpuixRequired, type HostElement, type KeyEvent, type EventPayload } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { agentSurface, type WorkspaceModel, type Surface } from "../model";
import { coreCall } from "../native-core";
import { writeClipboard } from "../clipboard";
import { PaintLayer, type PaintCommand, type PreparedCanvas } from "../ui/PaintLayer";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusNativeElement,invalidateNativeFocus } from "../native-focus";
import {JsonRenderControls,type JsonUiFrame,type JsonUiEvent,type JsonUiEventResult} from "./JsonRenderControls";
import {profileStart,profileEnd} from "../profiling";
import type {AgentFamilyRow} from "./family";
import { horizontalScrollEvent, readingScrollY, wheelCellPixels } from "../ui/wheel";
import {localPresenceIdentity} from "./local-presence";
import {handleApplicationShortcut} from "../commands/application-shortcuts";
import {paintedChromeBounds} from "../ui/chrome-target-bounds";
import {openExternalUrl} from "../menu-actions/web-frontend";
import {timelineReceivesEvent} from "./timeline-event-routing";
import {NativeViewLifetime} from "./native-view-lifetime";
import {RecentCanvasImages} from "../ui/canvas";

interface TimelineFrame { selecting?:boolean; preview?:{name:string;url:string}; canvas?: PreparedCanvas; commands: PaintCommand[]; animating: boolean; reason?:string; pointer?:boolean; copy?: string | null; link?: string | null;navigate?:string;send?:unknown[];header?:Array<{direction:string;target:string;rects:number[][]}> }
interface TimelineFrame {jsonUi?:JsonUiFrame;jsonUiAccepted?:boolean}
interface Engine { touched:number; id: number; workspaceId:string; surfaces:Set<string>; root:()=>string; images:Map<object,RecentCanvasImages>; lifetime:NativeViewLifetime; dirty: Set<() => void>; dispose: () => void }
const engineKey=(workspaceId:string,root:string)=>JSON.stringify([workspaceId,root]);

/** One original agent timeline engine per conversation (the shared Rust
 * `NeoismAgentPane`: cached layout, scroll-by-offset, stable-prefix
 * streaming, selection). It outlives tab switches like the desktop pane and
 * consumes the daemon's raw Agent events through the shared dispatcher. */
const positions=new WeakMap<WorkspaceModel,Map<string,{sessionId:string;offset:number;follow:boolean}>>();
const engineLimit=8;
function pruneEngines(model:WorkspaceModel){
 const entries=engines.get(model);if(!entries)return;
   const roots=new Set(model.state.surfaces.map(surface=>agentSurface(surface)).filter(s=>s.kind==="agent").flatMap(s=>{const root=s.sessionId?s.agentRootSessionId||model.agentFamily(s.sessionId).root:model.state.pendingAgentSessions[s.id];return root?[engineKey(s.workspaceId,root)]:[];}));
 for(const root of positions.get(model)?.keys()||[])if(!roots.has(root))positions.get(model)?.delete(root);
 for(const[root,engine]of entries)if(!roots.has(root)&&!engine.dirty.size){engine.dispose();positions.get(model)?.delete(root);}
 for(const[root,engine]of [...entries].sort((a,b)=>a[1].touched-b[1].touched)){
  if(entries.size<=engineLimit)break;if(engine.dirty.size)continue;
  const saved=positions.get(model)!;saved.set(root,coreCall("agenttimeline.position",{id:engine.id}));engine.dispose();
 }
}
const engines = new WeakMap<WorkspaceModel, Map<string, Engine>>();
function engineFor(model: WorkspaceModel, sessionId: string, config: AppConfig,rootSessionId=sessionId,workspaceId=model.state.activeWorkspace?.id||"",surfaceId?:string): Engine {
  let byModel = engines.get(model);
  if (!byModel) {
    byModel = new Map(); engines.set(model, byModel);positions.set(model,new Map());
    const off=model.subscribe(()=>pruneEngines(model));
    const offSurface=model.onSurfaceClosed((surface:Surface)=>{
       if(surface.kind!=="agent"&&surface.kind!=="welcome")return;
      for(const engine of [...byModel!.values()])if(engine.workspaceId===surface.workspaceId&&engine.surfaces.delete(surface.id)&&!engine.surfaces.size){positions.get(model)?.delete(engineKey(engine.workspaceId,engine.root()));engine.dispose();}
    });
    const offWorkspace=model.onWorkspaceClosed((workspaceId:string)=>{
      for(const engine of [...byModel!.values()])if(engine.workspaceId===workspaceId)engine.dispose();
      for(const key of positions.get(model)?.keys()||[])if((JSON.parse(key) as string[])[0]===workspaceId)positions.get(model)?.delete(key);
    });
    model.onDispose(()=>{off();offSurface();offWorkspace();for(const engine of [...byModel!.values()])engine.dispose();engines.delete(model);positions.delete(model);});
  }
  let key=engineKey(workspaceId,rootSessionId);
   const owners=()=>model.state.surfaces.map(surface=>agentSurface(surface)).filter(surface=>surface.workspaceId===workspaceId&&surface.kind==="agent"&&(surface.id===surfaceId||(surface.sessionId?(surface.agentRootSessionId||model.agentFamily(surface.sessionId).root):model.state.pendingAgentSessions[surface.id])===rootSessionId)).map(surface=>surface.id);
  const existing = byModel.get(key); if (existing) {existing.touched=performance.now();for(const id of owners())existing.surfaces.add(id);return existing;}
  const { id } = coreCall<{ id: number }>("agenttimeline.new", { compactPaint: true, canvasPaint: true, chunkedPaint: true, sessionId,rootSessionId, theme: config.themeName, scale: config.scale,
    fontFamily: config.fontFamily, markdownFontFamily: config.markdownFontFamily, fontWeight: config.fontWeight,reducedMotion:config.reducedMotion,localName:localPresenceIdentity(config).displayName });
  const family=model.agentFamily(sessionId);
  coreCall("agenttimeline.family",{id,deferFrame:true,rootSessionId,sessionId,roster:family.roster,rootTitle:model.state.threads.find(thread=>thread.session_id===rootSessionId)?.title||rootSessionId});
  const dirty = new Set<() => void>();
  const event = (message: unknown, historyKind?: "head" | "older" | "replace") => {
    if(lifetime?.closed)return;
    const adoption=(message as {LocalSessionAdopted?:{from:string;to:string}})?.LocalSessionAdopted;
    if(adoption){
      if(adoption.from===rootSessionId){
        coreCall("agenttimeline.adopt",{id,from:adoption.from,to:adoption.to});
        const engine=byModel!.get(key);byModel!.delete(key);rootSessionId=adoption.to;key=engineKey(workspaceId,rootSessionId);
        if(engine)byModel!.set(key,engine);
      }
      return;
    }
    // An evicted open family is still a known root. Do not broadcast its
    // transcript into every remaining engine just because it has no cache.
     const knownRoots={has:(root:string)=>byModel!.has(engineKey(workspaceId,root))||model.state.surfaces.map(surface=>agentSurface(surface)).some(surface=>surface.kind==="agent"&&!!surface.sessionId&&(surface.agentRootSessionId||model.agentFamily(surface.sessionId).root)===root)};
    if(!timelineReceivesEvent(rootSessionId,model.state.ui,knownRoots,message))return;
    const echo=(message as {MessageUpdated?:{session_id:string;local_echo?:boolean;message:{id:string}}})?.MessageUpdated;
    if(echo?.local_echo&&model.agentFamily(echo.session_id).root===rootSessionId)model.consumePromptArrival(echo.message.id);
    try { if(coreCall<{needsPaint?:boolean}>("agenttimeline.event", { id, message, historyKind }).needsPaint===false)return; } catch (error) { model.report(error); return; }
    for (const listener of dirty) listener();
  };
  let lifetime:NativeViewLifetime|undefined;
  // Seed what the model already holds; later daemon events stream in live.
  for(const selected of new Set([rootSessionId,sessionId,...family.roster.map(row=>row.id)])){
    const history=model.state.history[selected];if(history?.length)event({HistoryChunk:{session_id:selected,messages:history,next_cursor:model.historyCursors.get(selected)??null}},"head");
    const streaming=model.state.streaming[selected];if(streaming&&streaming!=="Idle")event({StreamingState:{session_id:selected,state:streaming,label:null}});
    for(const message of model.takePromptArrivals(selected))event({MessageUpdated:{session_id:selected,message,local_echo:true}});
  }
  const unsubscribe = model.subscribeAgentEvents(event);
  lifetime=new NativeViewLifetime(()=>{unsubscribe();if(byModel!.get(key)===engine)byModel!.delete(key);},()=>{for(const images of engine.images.values())images.dispose();engine.images.clear();coreCall("agenttimeline.dispose",{id});});
  const engine: Engine = { touched:performance.now(),id,workspaceId,surfaces:new Set(owners()),root:()=>rootSessionId,images:new Map(),lifetime, dirty, dispose: () => lifetime!.close() };
  byModel.set(key, engine);
  return engine;
}

/** Read the visible family's selection without creating an unseen timeline engine. */
export function agentSelectedText(model:WorkspaceModel,sessionId:string):string|undefined {
 const engine=engines.get(model)?.get(engineKey(model.state.activeWorkspace?.id||"",model.agentFamily(sessionId).root));
 return engine?coreCall<{text:string|null}>("agenttimeline.copy",{id:engine.id}).text||undefined:undefined;
}

/** Enter the original queue handoff immediately, before the abort round trip. */
export function stopAgentTimeline(model:WorkspaceModel,sessionId:string){
  const engine=engines.get(model)?.get(engineKey(model.state.activeWorkspace?.id||"",model.agentFamily(sessionId).root));if(!engine)return;
 coreCall("agenttimeline.stop",{id:engine.id,sessionId});
 for(const listener of engine.dirty)listener();
}

/** Route the original Agent half-page command while its composer owns focus. */
export function scrollAgentHalfPage(model:WorkspaceModel,sessionId:string,config:AppConfig,older:boolean){
 const engine=engineFor(model,sessionId,config,model.agentFamily(sessionId).root);
 coreCall("agenttimeline.half",{id:engine.id,older,deferFrame:true});
 for(const listener of engine.dirty)listener();
}

export function AgentTimelineView(props: { sessionId: string; model: WorkspaceModel; config: AppConfig; report: (error: unknown) => void; openFile: (path: string) => void;
  /** Explicit owner, including provisional conversations without a session yet. */
  workspaceId?:string;surfaceId?:string;
  /** Composer top: scroll metrics and sticky controls stop here; body paint continues behind the floating card. */
  viewport?:{width:number;height:number};pinImage?:(source:number,raster:CanvasRaster)=>void;onPreview?:(image:ImagePreview)=>void;provisional?:boolean;contentBottom?: number;rootSessionId?:string;familyRoster?:AgentFamilyRow[];onNavigate?:(sessionId:string)=>void;onFocus?:()=>void }) {
  const config = props.config, s = config.scale, renderer = useGpuixRequired();
  const localName=localPresenceIdentity(config).displayName;
  const engine = untrack(() => engineFor(props.model, props.sessionId, config,props.rootSessionId||props.sessionId,props.workspaceId??props.model.state.surfaces.find(surface=>surface.id===props.surfaceId)?.workspaceId??props.model.state.activeWorkspace?.id??"",props.surfaceId));
  // Raster leases follow the exact cached engine, not a window-wide pool: a
  // closed conversation must release them without evicting other workspaces.
  const pinImage=(source:number,raster:CanvasRaster)=>{
    if(engine.lifetime.closed)return;
    let images=engine.images.get(renderer);
    if(!images){images=new RecentCanvasImages(renderer,16*1024*1024);engine.images.set(renderer,images);}
    images.use(source,raster);
  };
  const [commands, setCommands] = createSignal<readonly PaintCommand[]>([]);
  const [canvas, setCanvas] = createSignal<PreparedCanvas>();
  const [header,setHeader]=createSignal<NonNullable<TimelineFrame["header"]>>([]);
  const [pointer,setPointer]=createSignal(false);
  const [selecting,setSelecting]=createSignal(false);
  const [jsonUi,setJsonUi]=createSignal<JsonUiFrame>();
  let bounds = { x: 0, y: 0, width: props.viewport?.width||0, height: props.viewport?.height||0 }, host: HostElement | undefined, disposed = false, startedAt = performance.now();
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const apply = (next: TimelineFrame) => {
    if(next.animating)profileEnd(`agent.animation.${next.reason||"scroll"}`,profileStart());
    batch(() => {setCommands(next.commands);setCanvas(next.canvas);setJsonUi(next.jsonUi);setPointer(!!next.pointer);setSelecting(!!next.selecting);setHeader(previous=>JSON.stringify(previous)===JSON.stringify(next.header||[])?previous:next.header||[]);});
    if(!props.provisional)for(const message of next.send||[])props.model.sendAgent(message as never);
    if(next.preview){
      const match=/^data:image\/[^;,]+;base64,(.*)$/s.exec(next.preview.url);
      if(match)props.onPreview?.({name:next.preview.name,bytes:Buffer.from(match[1],"base64")});
    }
    if(next.navigate)props.onNavigate?.(next.navigate);
    if (next.copy) void writeClipboard(next.copy).catch(props.report);
    if (next.link) {
      const target = next.link;
      if (/^(https?|mailto|tel):/i.test(target) || /^neoism:\/\/docs(?:[/?#]|$)/i.test(target)) void openExternalUrl(target).catch(props.report);
      else if (!target.startsWith("#")) props.openFile(target.split("#")[0]);
    }
    if (next.animating) schedule();
  };
  let pointerPending:{x:number;y:number}|undefined;
  const flushPointer=()=>{const point=pointerPending;pointerPending=undefined;if(point)coreCall("agenttimeline.pointer",{id:engine.id,kind:"move",deferFrame:true,...point});};
  const call = (op: string, args: Record<string, unknown> = {}) => {
    if(disposed||engine.lifetime.closed)return;
    startedAt = performance.now(); animation.cancel();
    try { flushFamily();flushPointer();apply(coreCall<TimelineFrame>(`agenttimeline.${op}`, { id: engine.id,...(op==="layout"?{localName}:{}), ...args })); } catch (error) { props.report(error); }
  };
  let layoutSignature = "";
  const widgetEvent=(event:JsonUiEvent):JsonUiEventResult=>{
    if(engine.lifetime.closed)return {accepted:false};
    try {
      flushFamily();flushPointer();
      const next=coreCall<TimelineFrame>("agenttimeline.json-ui",{id:engine.id,...event});
      if(!disposed)apply(next);
      if(next.jsonUiAccepted)for(const listener of engine.dirty)if(listener!==schedule)listener();
      return {accepted:next.jsonUiAccepted===true,frame:next.jsonUi};
    }catch(error){if(!disposed)props.report(error);return {accepted:false};}
  };
  const familyArgs=()=>({rootSessionId:props.rootSessionId||props.sessionId,sessionId:props.sessionId,roster:props.familyRoster||[],rootTitle:props.model.state.threads.find(thread=>thread.session_id===(props.rootSessionId||props.sessionId))?.title||props.rootSessionId||props.sessionId});
  // Select the requested member in the mounting transaction. Solid effects
  // run later; waiting for one exposes the previously viewed child's frame.
  let pendingFamily:Record<string,unknown>|undefined=untrack(familyArgs);
  const flushFamily=()=>{if(pendingFamily&&bounds.width>0&&bounds.height>0){const family=pendingFamily;pendingFamily=undefined;coreCall("agenttimeline.family",{id:engine.id,...family,deferFrame:true});}};
  const layout = (contentBottom = props.contentBottom ?? bounds.height) => {
    if(engine.lifetime.closed)return;
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const signature = `${bounds.width}:${bounds.height}:${contentBottom}:${config.reducedMotion}`;
    if (signature === layoutSignature) return;
    layoutSignature = signature;
    batch(()=>{coreCall("agenttimeline.layout",{id:engine.id,visible:true,width:bounds.width,height:bounds.height,contentBottom,scale:s,theme:config.themeName,reducedMotion:config.reducedMotion,localName,deferFrame:true});call("frame");});
  };
  // One paint per display frame however many events or animations arrive.
  const schedule = () => { if (!disposed&&!engine.lifetime.closed) animation.request(() => { if (!disposed && bounds.width > 0) call("frame"); }, startedAt); };
  const detach=engine.lifetime.attach(()=>{animation.dispose();pointerPending=undefined;});
  engine.dirty.add(schedule);
  onCleanup(() => {
    disposed=true;engine.dirty.delete(schedule);animation.dispose();
    if(!engine.dirty.size&&!engine.lifetime.closed)try{coreCall("agenttimeline.layout",{id:engine.id,visible:false,contentBottom:props.contentBottom??bounds.height,deferFrame:true});}catch{}
    detach();
    pruneEngines(props.model);
  });
  onMount(() => schedule());
  let familySignature=JSON.stringify(pendingFamily),selectedSession=props.sessionId;
  createEffect(()=>{const family=familyArgs();const signature=JSON.stringify(family);if(signature!==familySignature){familySignature=signature;untrack(()=>{pendingFamily=family;if(selectedSession!==props.sessionId){selectedSession=props.sessionId;pointerPending=undefined;call("frame");}else schedule();});}});
  createEffect(() => { const bottom = props.contentBottom; untrack(() => layout(bottom ?? bounds.height)); });
  const local = (event: {x?:number;y?:number}) => {
    // Read the completed paint snapshot, including moves without resize events.
    const painted=host&&paintedChromeBounds(renderer,host.id);
    if(painted){bounds={...bounds,x:painted.x,y:painted.y};}
    return {x:(event.x||0)-bounds.x,y:(event.y||0)-bounds.y};
  };
  const movePointer=(event:EventPayload)=>{
    if(selecting()&&event.pressedButton!==0){call("pointer",{kind:"up",...local(event)});return;}
    pointerPending=local(event);schedule();
  };
  const releasePointer=(event:EventPayload)=>{if(event.button===undefined||event.button===0)call("pointer",{kind:"up",...local(event)});};
  // Repeated navigation keys share the same display-frame scroll commit as
  // trackpad events. Rendering here as well doubled work between display wakes.
  const scroll = (deltaY:number) => {
    if(engine.lifetime.closed)return;
    try { coreCall("agenttimeline.scroll",{id:engine.id,deferFrame:true,deltaY,precise:true}); }
    catch(error){props.report(error);}
    schedule();
  };
  const keyDown = (event: KeyEvent) => {
    if(engine.lifetime.closed)return;
    if(handleApplicationShortcut(renderer,event))return;
    const key = event.key || "", ctrl = !!event.modifiers?.ctrl, modified = ctrl || !!event.modifiers?.alt || !!event.modifiers?.cmd;
    const handled = () => { event.preventDefault(); event.stopPropagation(); };
    if ((ctrl || event.modifiers?.cmd) && key.toLowerCase() === "c") {
      flushPointer();
      const text = coreCall<{ text: string | null }>("agenttimeline.copy", { id: engine.id }).text;
      if (text) { handled(); void writeClipboard(text).catch(props.report); }
    } else if (ctrl && !event.modifiers?.alt && !event.modifiers?.cmd && !event.modifiers?.shift && ["d", "u"].includes(key)) { handled(); call("half", { older: key === "u" }); }
    else if (["pageup", "pagedown"].includes(key)) { handled(); scroll((key === "pageup" ? 1 : -1) * bounds.height * 0.9); }
    else if (key === "home" || key === "end") { handled(); scroll(key === "home" ? 1e7 : -1e7); }
    else if (config.vim && !modified && ["j", "k"].includes(key)) { handled(); scroll((key === "k" ? 1 : -1) * 19 * s); }
  };
  // Seed paint at the restored pane size in the mounting transaction. Never
  // expose the engine's default/previous geometry while waiting for onLayout.
  layout();
   const positionKey=engineKey(engine.workspaceId,engine.root());
   const savedPosition=positions.get(props.model)?.get(positionKey);
   if(savedPosition&&savedPosition.sessionId===props.sessionId){coreCall("agenttimeline.restore-position",{id:engine.id,...savedPosition});positions.get(props.model)?.delete(positionKey);call("frame");}
  pruneEngines(props.model);
  return <div ref={host} testId="conversation" data-preserve-keyboard-focus={true} tabIndex={0} aria-label="Conversation history" onKeyDown={keyDown}
    onFocus={()=>{invalidateNativeFocus(renderer);props.onFocus?.();}}
    onBlur={()=>invalidateNativeFocus(renderer)}
    onLayout={(event) => {
      const next = { x: event.x || 0, y: event.y || 0, width: event.width || 0, height: event.height || 0 };
      const resized = next.width !== bounds.width || next.height !== bounds.height; bounds = next;
      if (resized) layout();
    }}
    onScroll={(event) => {
      if(engine.lifetime.closed)return;
      const wheel = horizontalScrollEvent(event);
      try { coreCall("agenttimeline.scroll", { id: engine.id, deferFrame: true, deltaX: wheel.deltaX || 0, deltaY: readingScrollY(wheel), precise: !!event.precise, ...(typeof event.x==="number"&&typeof event.y==="number"?local(event):{}), shift:!!event.modifiers?.shift, lineHeight: wheelCellPixels(event, config.fontSize) }); } catch (error) { props.report(error); }
      schedule();
    }}
    onMouseDown={(event) => { const point=local(event);if (event.button === 0) call("pointer", { kind: "down", ...point }); }}
    onMouseMove={movePointer}
    onMouseUp={releasePointer}
    onMouseLeave={() => call("pointer", { kind: "leave" })}
    style={{ flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative", overflow: "hidden", userSelect: "none", cursor: pointer()?"pointer":"text" }}>
    <Show when={selecting()}><anchored fill="window" deferred priority={3} occlude style={{backgroundColor:"transparent"}}>
      <div testId="timeline-selection-capture" data-preserve-keyboard-focus={true}
        onMouseMove={event=>{movePointer(event);}}
        onMouseUp={event=>{releasePointer(event);}}
        style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",cursor:"text",pointerEvents:"auto"}}/>
    </anchored></Show>
    <PaintLayer commands={commands()} prepared={canvas()} config={config} pinImage={pinImage} />
    <JsonRenderControls frame={jsonUi()} config={config} onEvent={widgetEvent}
      onChartPointer={(kind,event)=>{if(kind==="move")movePointer(event);else if(kind==="up")releasePointer(event);else call("pointer",{kind,...local(event)});}}
      onExit={()=>{if(host)focusNativeElement(renderer,host.id);}}/>
    <For each={header()}>{link=><For each={link.rects}>{rect=><div testId={`agent-family-navigation:${link.direction}`} aria-label={link.direction==="next"?"Next conversation":"Previous conversation"} style={{position:"absolute",left:rect[0],top:rect[1],width:rect[2],height:rect[3],pointerEvents:"none"}}/>}</For>}</For>
  </div>;
}
