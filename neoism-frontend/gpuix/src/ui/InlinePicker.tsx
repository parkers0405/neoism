import {openExternalUrl} from "../menu-actions/web-frontend";
import {handleApplicationShortcut} from "../commands/application-shortcuts";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { batch, createEffect, createSignal, Index, onCleanup, onMount, Show, untrack } from "solid-js";
import { createWindowSize,useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import { dimensions as d } from "../design/generated";
import { PaintLayer, type PaintCommand } from "./PaintLayer";
import {useOverlayViewport} from "./overlay-viewport";

export type Anchor = [number, number, number, number];
export interface PickerOption { title: string; value: string; description?: string; footer?: string; section?: string; header?: boolean; current?: boolean; pinned?: boolean; index?: number; mcp?: unknown }
/** Connect / OAuth / MCP stage status drawn by the original picker card. */
export interface PickerActivity { body: string; status: string; busy?: boolean; url?: string }
export interface PickerFrame {
  title: string; query: string; rows: PickerOption[]; selected: number; selectedValue?: string | null;
  top: number; offset: number; cursorOffset: number; rect: Anchor; limit: number; animating: boolean;
  loading?: boolean; responding?: boolean; current?: number; count?: number; answers?: string[][];
}
export function InlinePicker(props: {
  config: AppConfig; kind: string; title?: string; options?: readonly PickerOption[]; current?: string | null;
  question?: Record<string, unknown>; anchor: Anchor; loading?: boolean; error?: string; responding?: boolean;
  entry?:boolean; secret?:boolean; initialValue?:string; placeholder?:string; query?: string; onAcceptHandler?: (handler:(()=>void)|undefined)=>void; onKeyHandler?: (handler: ((event: KeyEvent) => boolean) | undefined) => void;
   onSelect?: (value: string) => void; onAnswers?: (answers: string[][]) => void; onClose: () => void;
   onBack?: () => void; providerRows?: boolean;
  ignoreOutsideClick?: (x: number, y: number) => boolean;
  activity?: PickerActivity; mcpServer?: string; mcpEntry?: unknown;
  usage?:{accounts:readonly {connection_id:string;label:string}[]};onRefresh?:()=>void|Promise<void>;onLoadMore?:()=>void|Promise<void>;
}) {
  const config = props.config, s = config.scale, renderer = useGpuixRequired();
  const viewport=useOverlayViewport(renderer,createWindowSize());
  const placement=()=>{const view=viewport(),anchor=props.anchor;const width=Math.min(anchor[2],view.width),x=Math.max(view.x,Math.min(anchor[0],view.x+view.width-width));return{anchor:[x,Math.max(view.y,Math.min(anchor[1],view.y+view.height)),width,anchor[3]],minY:view.y+8*s};};
  const engine: "question" | "picker" = props.question ? "question" : "picker" as "question" | "picker";
  // Both engines paint the original input-anchored Rust card. The host only
  // supplies a typing target and semantic/hit elements at measured row bounds.
  const native = engine === "picker";
  const nativeKind = props.entry ? (props.secret ? "secret" : "input") : props.kind;
  const id = coreCall<{ id: number }>(`${engine}.new`, { request: props.question, kind: nativeKind, title: props.title,
    options: props.options, selected: Math.max(0, props.options?.findIndex((o) => o.value === props.current) ?? 0), loading: props.loading,
    mcpServer: props.mcpServer, mcpEntry: props.mcpEntry, placeholder:props.placeholder }).id;
  const [card, setCard] = createSignal<{ commands: PaintCommand[]; rect?: Anchor | null; backRect?: Anchor; rows?: Array<{index:number;rect:Anchor}> }>({ commands: [] });
  const [frame, setFrame] = createSignal<PickerFrame>(coreCall(`${engine}.frame`, { id, ...placement(), scale: s }));
  let input: HostElement | undefined;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let disposed = false;
  const update = (op: string, args: Record<string, unknown> = {}) => {
    const started = performance.now(); animation.cancel();
    const next = coreCall<PickerFrame>(`${engine}.${op}`, { id, ...placement(), scale: s, entry:props.entry, footer: props.kind === "permission" && !!(props.responding || props.error), ...args });
    let animating = next.animating;
    {
      const painted = coreCall<{ commands: PaintCommand[]; rect?: Anchor | null; backRect?: Anchor; rows?: Array<{index:number;rect:Anchor}>; animating: boolean }>(`${engine}.paint`, { id, ...placement(), scale: s,
        theme: config.themeName, fontFamily: config.fontFamily, fontWeight: config.fontWeight, caret: props.query === undefined, error:props.error, back: !!props.onBack, providerRows: props.providerRows || props.kind === "connect" && !props.activity });
      batch(()=>{setFrame(next);setCard(painted);}); animating ||= painted.animating;
    }
    if (animating) animation.request(() => { if (!disposed) update("frame"); }, started);
    // Only user navigation may fetch another page, never paints or page appends.
    if((op==="scroll"&&Number(args.delta)<0||op==="move"&&Number(args.delta)>0)&&!props.loading&&Math.max(next.top,next.selected)+next.limit>=(next.count||0)-3)void props.onLoadMore?.();
    return next;
  };
  // Track only each effect's inputs. Painting reads query, geometry and other
  // props; allowing those reads to escape would resend the full catalog on
  // every query edit or layout change. Initial effects share one paint.
  let paintQueued=false;
  const refresh=()=>{if(paintQueued)return;paintQueued=true;queueMicrotask(()=>{paintQueued=false;if(!disposed)untrack(()=>update("frame"));});};
  const mutate=(op:string,args:Record<string,unknown>)=>untrack(()=>{coreCall(`${engine}.${op}`,{id,...args,deferFrame:true});refresh();});
  createEffect(()=>{if(native){const a=props.activity;mutate("activity",a?{body:a.body,status:a.status,busy:!!a.busy,url:a.url}:{});}});
  createEffect(()=>{viewport();props.anchor;props.responding;props.error;refresh();});
  createEffect(()=>{if(engine==="picker")mutate("options",{options:props.options||[],loading:props.loading,title:props.title,placeholder:props.placeholder});});
  createEffect(()=>{if(props.kind==="usage")mutate("usage",{usage:props.usage,loading:props.loading,error:props.error});});
  createEffect(()=>{if(engine==="question"&&props.error)untrack(()=>update("retry"));});
  createEffect(()=>{const query=props.query;if(query!==undefined&&engine==="picker")mutate("query",{query});});
  const focusInput = () => queueMicrotask(() => {
    if (!disposed && input && props.query === undefined) {
      input.root?.mutations.flushMutations();
      renderer.focusElement?.(input.id);
    }
  });
  onMount(() => { if(props.initialValue!==undefined)update("query",{query:props.initialValue}); focusInput(); });
  onCleanup(() => { props.onKeyHandler?.(undefined); props.onAcceptHandler?.(undefined); disposed = true; animation.dispose(); coreCall(`${engine}.dispose`, { id }); });
  const accept = (index?: number) => {
    if (props.responding || frame().responding) return;
    if(props.entry&&engine==="picker"){props.onSelect?.(frame().query);return;}
    if (index !== undefined) update("select", { index });
    if (engine === "question") {
      const next = update("commit"); if (next.answers) props.onAnswers?.(next.answers);
      // A clicked suggestion can advance to a free-text question in this same input.
      focusInput();
    } else {
      const value = frame().selectedValue;
      if (value !== undefined && value !== null) props.onSelect?.(value);
    }
  };
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return true;
    let handled = true;
    const key = event.key?.toLowerCase().replace(/^arrow/, "") || "", ctrl = event.modifiers?.ctrl;
    if(props.kind==="usage"&&key==="enter"){event.preventDefault();event.stopPropagation();if(!props.loading)void props.onRefresh?.();}
    else if (key === "escape" || props.onBack && key === "left" && (event.modifiers?.alt || !frame().query)) { event.preventDefault(); event.stopPropagation(); if (!props.responding) (props.onBack || props.onClose)(); }
    else if (["up", "down"].includes(key) || ctrl && ["n", "p"].includes(key)) {
      event.preventDefault(); event.stopPropagation(); update("move", { delta: key === "up" || key === "p" ? -1 : 1 });
    } else if (key === "enter") { event.preventDefault(); event.stopPropagation(); accept(); }
    else if (ctrl && key === "u" && props.query === undefined) { event.preventDefault(); event.stopPropagation(); update("query", { query: "" }); }
    else handled = false;
    return handled;
  };
  onMount(() => {props.onKeyHandler?.(keyDown);props.onAcceptHandler?.(()=>{if(props.kind==="usage"){if(!props.loading)void props.onRefresh?.();}else accept();});});
    const rect = () => card().rect || frame().rect;
    const hit = (event: { x?: number; y?: number }) => coreCall<{ hit?: boolean; inside?: boolean; url?: string }>("picker.hit", { id, x: event.x || 0, y: event.y || 0 });
    return <anchored fill="window" deferred={true} priority={1} occlude={false} style={{ backgroundColor: "transparent", pointerEvents: "none" }}>
      <PaintLayer commands={card().commands} config={config} />
      <div testId={native ? `agent-picker:${props.kind}` : "agent-question"} role="dialog" aria-label={frame().title} aria-description={props.error} onKeyDown={keyDown}
        onMouseDownOutside={event => { const [x,y,w,h]=rect(); if(event.x!==undefined&&event.y!==undefined&&(props.ignoreOutsideClick?.(event.x,event.y)||event.x>=x-1&&event.y>=y-1&&event.x<=x+w+1&&event.y<=y+h+1))return; if(!props.responding)props.onClose(); }}
        onMouseDown={(event) => { if(!native)return; const result = hit(event); if (result.url) { void openExternalUrl(result.url).catch(error=>console.error("Could not open authorization link",error)); } else if (result.hit) { update("frame"); accept(); } }}
        onScroll={(event) => {const delta=(event.deltaY || 0) * (event.precise ? 1 / s : d.pickerRowHeight * 3);if(native)update("scroll",{delta});else if(delta)update("move",{delta:delta>0?-1:1});}}
        style={{ position: "absolute", left: rect()[0], top: rect()[1], width: rect()[2], height: rect()[3], pointerEvents: "auto", userSelect: "none" }}>
        <Show when={props.onBack && card().backRect}>{bounds => <div role="button" tabIndex={0} testId="picker-back" aria-label="Back to providers or previous screen"
          onClick={() => {if(!props.responding)props.onBack?.();}}
          onKeyDown={event => {if(["enter"," ","space"].includes(event.key?.toLowerCase() || "")){event.preventDefault();event.stopPropagation();if(!props.responding)props.onBack?.();}}}
          style={{position:"absolute",left:bounds()[0]-rect()[0],top:bounds()[1]-rect()[1],width:bounds()[2],height:bounds()[3],cursor:"pointer",pointerEvents:"auto"}} />}</Show>
        <Index each={frame().rows}>{(option,slot) => {
          const index=()=>option().index??slot;
          const bounds=()=>card().rows?.find(row=>row.index===index())?.rect;
          return <div role="option" aria-label={option().title} aria-selected={frame().selected===index()} data-preserve-keyboard-focus={true}
            testId={props.secret ? "picker-option:credential" : `picker-option:${option().value}`}
            onMouseDown={()=>{if(!native&&!option().header)accept(index());}}
            style={{position:"absolute",left:(bounds()?.[0]||0)-rect()[0],top:(bounds()?.[1]||0)-rect()[1],width:bounds()?.[2]||0,height:bounds()?.[3]||0,pointerEvents:!native&&bounds()?"auto":"none",cursor:"pointer"}} />;
        }}</Index>
        <Show when={props.kind==="usage"}><Index each={props.usage?.accounts||[]}>{account=><div role="listitem" testId={`usage-account:${account().connection_id}`} aria-label={account().label} style={{width:0,height:0,pointerEvents:"none"}}/>}</Index></Show>
        <Show when={props.query === undefined}>
          {/* Typing target only; the original card paints query, caret and mask. */}
          <input ref={input} testId="inline-picker-input" onKeyDown={keyDown} autoFocus={true} aria-label={native ? `Search ${props.title || "options"}` : "Answer question"} value={frame().query}
            onChange={(event) => {if(props.kind!=="usage")update("query", { query: event.value || "" });}} onSubmit={() => {if(props.kind==="usage"){if(!props.loading)void props.onRefresh?.();}else accept();}}
            style={{ position: "absolute", left: 0, top: 0, width: 1, height: 1, opacity: 0, color: "transparent", backgroundColor: "transparent", borderWidth: 0, pointerEvents: "none" }} />
        </Show>
      </div>
    </anchored>;
}
