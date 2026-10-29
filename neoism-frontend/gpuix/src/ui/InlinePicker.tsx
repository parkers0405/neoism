import {handleApplicationShortcut} from "../commands/application-shortcuts";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { createEffect, createSignal, Index, onCleanup, onMount, Show } from "solid-js";
import { createWindowSize,useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import { dimensions as d } from "../design/generated";
import { LoadingSkeleton } from "./LoadingSkeleton";
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
  entry?:boolean; secret?:boolean; initialValue?:string; query?: string; onAcceptHandler?: (handler:(()=>void)|undefined)=>void; onKeyHandler?: (handler: ((event: KeyEvent) => boolean) | undefined) => void;
  onSelect?: (value: string) => void; onAnswers?: (answers: string[][]) => void; onClose: () => void;
  activity?: PickerActivity; mcpServer?: string; mcpEntry?: unknown;
  usage?:{accounts:readonly {connection_id:string;label:string}[]};onRefresh?:()=>void|Promise<void>;
}) {
  const config = props.config, c = config.theme, s = config.scale, renderer = useGpuixRequired();
  const viewport=useOverlayViewport(renderer,createWindowSize());
  const placement=()=>{const view=viewport(),anchor=props.anchor;const width=Math.min(anchor[2],view.width),x=Math.max(view.x,Math.min(anchor[0],view.x+view.width-width));return{anchor:[x,Math.max(view.y,Math.min(anchor[1],view.y+view.height)),width,anchor[3]],minY:view.y+8*s};};
  const engine: "question" | "picker" = props.question ? "question" : "picker" as "question" | "picker";
  // List pickers are the original Rust inline-picker card (desktop parity:
  // rounded inset highlight, current/pinned marks, MCP status + artwork,
  // connect/OAuth activity, loading skeleton). Questions keep their view.
  const native = engine === "picker";
  const nativeKind = props.entry ? (props.secret ? "secret" : "input") : props.kind;
  const id = coreCall<{ id: number }>(`${engine}.new`, { request: props.question, kind: nativeKind, title: props.title,
    options: props.options, selected: Math.max(0, props.options?.findIndex((o) => o.value === props.current) ?? 0), loading: props.loading,
    mcpServer: props.mcpServer, mcpEntry: props.mcpEntry }).id;
  const [card, setCard] = createSignal<{ commands: PaintCommand[]; rect?: Anchor | null }>({ commands: [] });
  const [headerExtra, setHeaderExtra] = createSignal(0);
  const [frame, setFrame] = createSignal<PickerFrame>(coreCall(`${engine}.frame`, { id, ...placement(), scale: s }));
  let input: HostElement | undefined;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let disposed = false;
  const update = (op: string, args: Record<string, unknown> = {}) => {
    const started = performance.now(); animation.cancel();
    const next = coreCall<PickerFrame>(`${engine}.${op}`, { id, ...placement(), scale: s, headerExtra: headerExtra(), entry:props.entry, footer: props.kind === "permission" && !!(props.responding || props.error), ...args });
    setFrame(next);
    let animating = next.animating;
    if (native) {
      const painted = coreCall<{ commands: PaintCommand[]; rect?: Anchor | null; animating: boolean }>("picker.paint", { id, ...placement(), scale: s,
        theme: config.themeName, fontFamily: config.fontFamily, fontWeight: config.fontWeight, caret: props.query === undefined });
      setCard(painted); animating ||= painted.animating;
    }
    if (animating) animation.request(() => { if (!disposed) update("frame"); }, started);
    return next;
  };
  createEffect(() => { if (native) { const a = props.activity; coreCall("picker.activity", { id, ...(a ? { body: a.body, status: a.status, busy: !!a.busy, url: a.url } : {}) }); update("frame"); } });
  createEffect(() => { props.anchor; headerExtra(); update("frame"); });
  createEffect(() => { if (engine === "picker") update("options", { options: props.options || [], loading: props.loading }); });
  createEffect(()=>{if(props.kind==="usage")update("usage",{usage:props.usage,loading:props.loading,error:props.error});});
  createEffect(() => { if (engine === "question" && props.error) update("retry"); });
  createEffect(() => { if (props.query !== undefined && engine === "picker") update("query", { query: props.query }); });
  onMount(() => { if(props.initialValue!==undefined)update("query",{query:props.initialValue}); if (input && props.query === undefined) renderer.focusElement?.(input.id); });
  onCleanup(() => { props.onKeyHandler?.(undefined); props.onAcceptHandler?.(undefined); disposed = true; animation.dispose(); coreCall(`${engine}.dispose`, { id }); });
  const accept = (index?: number) => {
    if (props.responding || frame().responding) return;
    if (index !== undefined) update("select", { index });
    if (engine === "question") {
      const next = update("commit"); if (next.answers) props.onAnswers?.(next.answers);
    } else {
      const value = frame().selectedValue;
      if (value !== undefined && value !== null) props.onSelect?.(value);
    }
  };
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return true;
    let handled = true;
    const key = event.key || "", ctrl = event.modifiers?.ctrl;
    if(props.kind==="usage"&&key==="enter"){event.preventDefault();event.stopPropagation();if(!props.loading)void props.onRefresh?.();}
    else if (key === "escape") { event.preventDefault(); event.stopPropagation(); if (!props.responding) props.onClose(); }
    else if (["up", "down"].includes(key) || ctrl && ["n", "p"].includes(key)) {
      event.preventDefault(); event.stopPropagation(); update("move", { delta: key === "up" || key === "p" ? -1 : 1 });
    } else if (key === "enter" && props.query !== undefined) { event.preventDefault(); event.stopPropagation(); accept(); }
    else if (ctrl && key === "u" && props.query === undefined) { event.preventDefault(); event.stopPropagation(); update("query", { query: "" }); }
    else handled = false;
    return handled;
  };
  onMount(() => {props.onKeyHandler?.(keyDown);props.onAcceptHandler?.(()=>{if(props.kind==="usage"){if(!props.loading)void props.onRefresh?.();}else accept();});});
  const text = (size: number, color = c.fg) => ({ color, fontFamily: config.fontFamily, fontSize: size * s, fontWeight: config.fontWeight, pointerEvents: "none" as const });
  const headerHeight = () => d.pickerTitleHeight * s + headerExtra();
  const footerHeight = () => !props.entry && engine === "question" || props.kind === "permission" && (props.responding || props.error) ? d.pickerFooterHeight * s : 0;
  const rowHeight = d.pickerRowHeight * s;
  if (native) {
    const rect = () => card().rect || frame().rect;
    const hit = (event: { x?: number; y?: number }) => coreCall<{ hit?: boolean; inside?: boolean; url?: string }>("picker.hit", { id, x: event.x || 0, y: event.y || 0 });
    return <anchored fill="window" deferred={true} priority={1} occlude={false} style={{ backgroundColor: "transparent", pointerEvents: "none" }}>
      <PaintLayer commands={card().commands} config={config} />
      <div testId={`agent-picker:${props.kind}`} role="dialog" aria-label={frame().title} onKeyDown={keyDown}
        onMouseDownOutside={event => { const [x,y,w,h]=rect(); if(event.x!==undefined&&event.y!==undefined&&event.x>=x-1&&event.y>=y-1&&event.x<=x+w+1&&event.y<=y+h+1)return; if(!props.responding)props.onClose(); }}
        onMouseDown={(event) => { const result = hit(event); if (result.url) { const child = Bun.spawn([process.platform === "darwin" ? "open" : "xdg-open", result.url], { stderr: "ignore", stdout: "ignore" }); void child.exited; } else if (result.hit) { update("frame"); accept(); } }}
        onScroll={(event) => update("scroll", { delta: (event.deltaY || 0) * (event.precise ? 1 / s : d.pickerRowHeight * 3) })}
        style={{ position: "absolute", left: rect()[0], top: rect()[1], width: rect()[2], height: rect()[3], pointerEvents: "auto", userSelect: "none" }}>
        <Index each={frame().rows}>{(option) => <div role="option" aria-label={option().title} testId={props.secret ? "picker-option:credential" : `picker-option:${option().value}`} style={{ position: "absolute", width: 0, height: 0, pointerEvents: "none" }} />}</Index>
        <Show when={props.kind==="usage"}><Index each={props.usage?.accounts||[]}>{account=><div role="listitem" testId={`usage-account:${account().connection_id}`} aria-label={account().label} style={{width:0,height:0,pointerEvents:"none"}}/>}</Index></Show>
        <Show when={props.query === undefined}>
          {/* Typing target only; the original card paints query, caret and mask. */}
          <input ref={input} testId="inline-picker-input" autoFocus={true} aria-label={`Search ${props.title || "options"}`} value={frame().query}
            onChange={(event) => {if(props.kind!=="usage")update("query", { query: event.value || "" });}} onSubmit={() => {if(props.kind==="usage"){if(!props.loading)void props.onRefresh?.();}else accept();}}
            style={{ position: "absolute", left: 0, top: 0, width: 1, height: 1, opacity: 0, color: "transparent", backgroundColor: "transparent", borderWidth: 0, pointerEvents: "none" }} />
        </Show>
      </div>
    </anchored>;
  }
  return <anchored fill="window" deferred={true} priority={1} occlude={false} style={{ backgroundColor: "transparent", pointerEvents: "none" }}><div testId={engine === "question" ? "agent-question" : `agent-picker:${props.kind}`} role="dialog" aria-label={frame().title}
    onKeyDown={keyDown} onMouseDownOutside={event => {
      // Deferred popups can move between the input event and the JS drain.
      // A click inside their current source geometry must never reject a
      // permission/question because the preceding native hit tree was stale.
      const [x,y,w,h]=frame().rect;
      if(event.x!==undefined&&event.y!==undefined&&event.x>=x-1&&event.y>=y-1&&event.x<=x+w+1&&event.y<=y+h+1)return;
      if(!props.responding)props.onClose();
    }}
    style={{ position: "absolute", left: frame().rect[0], top: frame().rect[1], width: frame().rect[2], height: frame().rect[3],
      backgroundColor: c.bg, borderRadius: d.pickerRadius * s, borderWidth: s, borderColor: c.border, overflow: "hidden", pointerEvents: "auto", userSelect: "none" }}>
    <text onLayout={(event) => { if (engine === "question") setHeaderExtra(Math.max(0, (event.height || 18 * s) + 36 * s - d.pickerTitleHeight * s)); }}
      style={{ ...text(13), position: "absolute", left: 14 * s, right: 48 * s, top: 9 * s,
        whiteSpace: engine === "question" ? "normal" : "nowrap", textOverflow: "ellipsis" }}>{frame().title}</text>
    <text onClick={props.onClose} style={{ ...text(12, c.muted), position: "absolute", right: 14 * s, top: 10 * s, cursor: "pointer", pointerEvents: "auto" }}>esc</text>
    <Show when={props.query === undefined} fallback={<text style={{ ...text(13), position: "absolute", left: 14 * s, right: 30 * s, top: headerHeight() - 24 * s, height: 22 * s }}>{props.query || "Search"}</text>}>
    <input ref={input} testId="inline-picker-input" autoFocus={true} aria-label={engine === "question" ? "Answer question" : `Search ${props.title || "options"}`}
      value={frame().query} placeholder={engine === "question" ? "Type your answer" : "Search"}
      onChange={(event) => {if(props.kind!=="usage")update("query", { query: event.value || "" });}} onSubmit={() => {if(props.kind==="usage"){if(!props.loading)void props.onRefresh?.();}else accept();}}
      style={{ ...text(13, props.secret ? "transparent" : c.fg), position: "absolute", left: 14 * s, width: Math.max(1, frame().rect[2] - 30 * s), top: headerHeight() - 24 * s, height: 22 * s,
        backgroundColor: c.bg, borderWidth: 0, userSelect: "text", pointerEvents: "auto" }} />
    </Show>
    <Show when={props.secret}><text style={{...text(13),position:"absolute",left:14*s,right:30*s,top:headerHeight()-24*s,height:22*s,pointerEvents:"none",whiteSpace:"nowrap",overflow:"hidden"}}>{"•".repeat([...frame().query].length)}</text></Show>
    <div onScroll={(event) => {
      const delta = (event.deltaY || 0) * (event.precise ? 1 / s : d.pickerRowHeight * 3);
      if ((engine as string) === "picker") update("scroll", { delta }); else if (delta) update("move", { delta: delta > 0 ? -1 : 1 });
    }} style={{ position: "absolute", top: headerHeight(), left: 0, right: 0, bottom: footerHeight(), overflow: "hidden" }}>
      <Show when={!props.entry&&frame().rows.length}><div style={{ position: "absolute", left: s, right: s,
        top: (frame().selected - frame().top) * rowHeight + (frame().offset + frame().cursorOffset) * s,
        height: rowHeight, backgroundColor: c.hover, pointerEvents: "none" }} /></Show>
      {/* Rows are the native visible window; `index` is the absolute position. */}
      <Index each={frame().rows}>{(option, slot) => { const index = () => option().index ?? slot; return <div role="option" aria-selected={frame().selected === index()} aria-label={props.secret?"Submit credentials":option().title}
        testId={props.secret?"picker-option:credential":`picker-option:${option().value}`} onMouseDown={() => { if (!option().header) accept(index()); }}
        style={{ position: "absolute", left: 0, right: 0, top: (index() - frame().top) * rowHeight + frame().offset * s,
          height: rowHeight, userSelect: "none", paddingLeft: 14 * s, paddingRight: 14 * s, cursor: option().header ? "default" : "pointer", hover: option().header ? undefined : { backgroundColor: c.hover } }}>
        <text style={{ ...text(13, option().header ? c.muted : c.fg), position: "absolute", left: 14 * s, right: 86 * s, top: 5 * s, whiteSpace: "nowrap", textOverflow: "ellipsis" }}>{option().current ? "● " : ""}{props.secret?"Submit credentials":option().title}</text>
        <text style={{ ...text(11, c.muted), position: "absolute", left: 14 * s, right: 86 * s, top: 25 * s, whiteSpace: "nowrap", textOverflow: "ellipsis" }}>{option().header ? "" : option().description || option().section || ""}</text>
        <text style={{ ...text(11, c.muted), position: "absolute", right: 14 * s, top: 9 * s }}>{option().footer || ""}</text>
      </div>; }}</Index>
      <Show when={!props.entry&&!frame().rows.length}><Show when={props.loading} fallback={<text style={{ ...text(12, c.muted), margin: 14 * s }}>{engine === "question" ? frame().responding ? "Sending answer…" : "Type your answer, then press enter" : props.error || "No results"}</text>}><LoadingSkeleton config={config} kind="picker" /></Show></Show>
    </div>
    <Show when={footerHeight() > 0}><text style={{ ...text(11, props.error ? c.red : c.muted), position: "absolute", left: 14 * s, right: 14 * s, bottom: 6 * s }}>
      {props.error || (props.responding || frame().responding ? "sending…" : "enter answer · esc skip")}
    </text></Show>
  </div></anchored>;
}
