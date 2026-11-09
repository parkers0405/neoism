import { createEffect, createMemo, createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useGpuixRequired, type HostElement, type KeyEvent, type EventPayload } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { DaemonConnection } from "../daemon/client";
import { terminalSession } from "./sessions";
import {TerminalCursorAnimation} from "./cursor-animation";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import type {TerminalCursorFrame} from "./buffer";
import {cssColor} from "../ui/canvas";
import { coreCall, type SearchHost } from "../native-core";
import { cursorPaint, sameBounds } from "../editor/paint-frame";
import { focusedNativeElement, focusNativeElement, invalidateNativeFocus } from "../native-focus";
import {CursorBlinkClock} from "../ui/cursor-blink";
import {terminalPaintOps} from "./paint";
import {profileStart,profileEnd} from "../profiling";
import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import { BlockTerminalView } from "./BlockTerminalView";
import { terminalInput } from "./input";

export interface TerminalAgentInput {
  text: string;
  resetToken: number;
  footerHeight: number;
  footer: JSX.Element;
  overlays: JSX.Element;
  onHost: (host: HostElement | undefined) => void;
  onBounds: (bounds: { x: number; y: number; width: number; height: number }) => void;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onKeyDown: (event: KeyEvent) => void;
  onFocus: () => void;
  onBlur: () => void;
}

export interface TerminalViewProps { onCustomKey?: (event: KeyEvent) => boolean; active?: boolean; focusToken?: number; onFocus?: () => void; sessionId: string; client: DaemonConnection; config: AppConfig; onSearch: (kind: "/" | "?", host: SearchHost) => void; report: (error: unknown) => void;
  onAttachBlock?: (block: { command: string; directory: string; output: string }) => void;
  onSaveCommand?: (command: string, directory: string) => void;
  savedCommands?: readonly string[];
  onOpenPath?: (path: string, isDirectory: boolean) => void;
  directoryAuthority?: boolean;
  onMakeDirectoryAuthority?: () => void;
  /** Home content participates in scrollback above a bottom-anchored composer. */
  historyHeader?: (viewport: { width: number; height: number }) => JSX.Element;
  agentMode?: boolean;
  agentInput?: TerminalAgentInput;
  agentHistory?: (viewport: { width: number; height: number }) => JSX.Element;
}

export function TerminalView(props: TerminalViewProps) {
  return <Show when={props.historyHeader || props.config.terminalBlockUi} fallback={<TerminalGridView {...props}/>}>
    <BlockTerminalView {...props} grid={() => <TerminalGridView {...props}/>}/>
  </Show>;
}

function TerminalGridView(props: TerminalViewProps) {
  const renderer = useGpuixRequired();
  const session = terminalSession(props.client, props.sessionId, props.config.theme, props.report), buffer = session.buffer;
  const [frame, setFrame] = createSignal(buffer.frame());
  const [size, setSize] = createSignal({ width: 0, height: 0, x: 0, y: 0 }, { equals: sameBounds });
  const [cellWidth, setCellWidth] = createSignal(props.config.fontSize * 0.6);
  const lineHeight = props.config.fontSize * 1.3;
  const [focused, setFocused] = createSignal(false);
  const active = createMemo(() => props.active !== false);
  const [blink, setBlink] = createSignal(false);
  const blinkClock=new CursorBlinkClock(setBlink,{scope:renderer});
  const [cursor, setCursor] = createSignal<TerminalCursorFrame>({ quad: [], hold: false,animating:false }, { equals: (a, b) => a.hold === b.hold && a.animating === b.animating && a.quad.length === b.quad.length && a.quad.every((point,i) => point.every((n,j) => n === b.quad[i][j])) });
  const [cursorColor, setCursorColor] = createSignal(props.config.cursorColor || props.config.theme.accent);
  const offset = createMemo(() => frame().offset);
  const ink=createMemo(()=>({lines:frame().lines,extra:frame().extra}),undefined,{equals:(a,b)=>a.lines===b.lines&&a.extra===b.extra});
  const paint=createMemo(()=>{const started=profileStart();try{return JSON.stringify(terminalPaintOps(ink(),props.config,cellWidth(),lineHeight));}finally{profileEnd("terminal.paint",started);}});
  const caret = createMemo(() => cursorPaint(cursor().quad));
  let host: HostElement | undefined, selecting = false, disposed = false;
  const send = session.send;
  const cursorAnimation=new TerminalCursorAnimation(()=>buffer.cursor(cellWidth(),lineHeight),setCursor,focused);
  const paintAnimation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  createEffect(()=>{const position=frame().cursor;blinkClock.update({active:focused(),enabled:props.config.cursorBlink,intervalMs:props.config.cursorBlinkInterval,animating:cursor().animating,mode:"terminal",activityToken:`${position.x}:${position.y}:${position.shape}:${cellWidth()}:${lineHeight}`});});
  const receiveFocus = (value: boolean) => {
    if (focused() === value) return;
    setFocused(value); buffer.focus(value);
    if (value) { props.onFocus?.(); cursorAnimation.request(); }
    else { selecting = false; cursorAnimation.cancel(); }
  };
  // Parse every packet immediately, but do not rebuild/upload a viewport for
  // every packet in an output burst. Present the latest state once per frame.
  const cleanupBuffer = buffer.subscribe(() => {paintAnimation.request(()=>{if(!disposed)setFrame(buffer.frame());});cursorAnimation.request();});
  createEffect(()=>{cellWidth();cursorAnimation.request();});
  const blinkTimer = setInterval(() => {
    const owner = focusedNativeElement(renderer);
    if (owner !== undefined && host?.id) receiveFocus(owner === host.id);
    if (!focused()) return;
    if (props.config.cursorStyle === "rainbow") {const style = coreCall<{rainbow: number[] }>("cursor", { seconds: performance.now() / 1000 });setCursorColor("#" + style.rainbow.slice(0, 3).map((n) => n.toString(16).padStart(2, "0")).join(""));}
  }, 1000 / 60);
  createEffect(() => {
    const { width, height } = size();
    if (width <= 0 || height <= 0) return;
    if (buffer.resize(Math.floor(width / cellWidth()), Math.floor(height / lineHeight), cellWidth(), lineHeight)) {
      try { props.client.send("Pty", { Resize: { session_id: props.sessionId, cols: buffer.cols, rows: buffer.rows } }); }
      catch (error) { props.report(error); }
    }
  });
  createEffect(() => { props.focusToken; if (active() && host) focusNativeElement(renderer, host.id); });
  let releaseCaret=()=>{};
  onMount(() => {if(host)releaseCaret=registerSurfaceCaret(renderer,host,()=>cursor().quad.map(point=>[point[0]+size().x,point[1]+size().y]));if (active() && host) focusNativeElement(renderer, host.id); });
  onCleanup(()=>releaseCaret());
  onCleanup(() => { disposed = true; paintAnimation.dispose(); cursorAnimation.dispose(); blinkClock.dispose(); buffer.focus(false); cleanupBuffer(); clearInterval(blinkTimer); });
  const keyDown = terminalInput({ buffer, send, report: props.report, custom: props.onCustomKey, search: props.onSearch,
    focus: () => { if (host) focusNativeElement(renderer, host.id); }, activity: () => blinkClock.activity(), disposed: () => disposed });
  const select = (x: number, y: number, start: boolean, clicks = 1) => {
    const left = size().x, top = size().y;
    const col = Math.max(0, Math.min(buffer.cols - 1, Math.floor((x - left) / cellWidth())));
    const row = Math.max(0, Math.min(buffer.rows - 1, Math.floor((y - top - frame().offset) / lineHeight)));
    buffer.select(row, col, start, x - left, cellWidth(), clicks);
  };
  const mouse = (event: EventPayload, kind: "down" | "move" | "up" | "wheel") => {
    const left = size().x, top = size().y;
    return buffer.mouse({ kind, row: Math.floor(((event.y ?? top) - top - frame().offset) / lineHeight), col: Math.floor(((event.x ?? left) - left) / cellWidth()),
      button: kind === "move" ? event.pressedButton : event.button, delta: event.deltaY || 0, precise: event.precise, rowHeight: lineHeight, cellWidth: cellWidth(), ...event.modifiers });
  };
  return <div ref={host} testId="terminal" tabIndex={0} role="terminal" aria-label="Terminal"
    onClick={() => { props.onFocus?.(); if (host) focusNativeElement(renderer, host.id); }}
    onMouseDown={(event) => { if(event.button===0){props.onFocus?.();if(host)focusNativeElement(renderer,host.id);} if (mouse(event, "down")) return; if (event.button === 0) { selecting = true; select(event.x || 0, event.y || 0, true, event.clickCount); } }}
    onMouseMove={(event) => { if (mouse(event, "move")) return; if (selecting && event.pressedButton === 0) select(event.x || 0, event.y || 0, false); }}
    onMouseUp={(event) => { mouse(event, "up"); selecting = false; }}
    onFocus={() => { invalidateNativeFocus(renderer); receiveFocus(true); }} onBlur={() => { invalidateNativeFocus(renderer); receiveFocus(false); }} onKeyDown={(event) => keyDown(event)} onKeyUp={(event) => keyDown(event, true)}
    onLayout={(event) => setSize({ width: event.width || 0, height: event.height || 0, x: event.x || 0, y: event.y || 0 })}
    onScroll={(event) => {
      if (mouse(event, "wheel")) return;
      buffer.scrollPixels((event.deltaY || 0) * (event.precise ? 1 : lineHeight) * (props.config.terminalScrollMultiplier ?? 3) / (props.config.terminalScrollDivider ?? 1), lineHeight);
    }}
    style={{ flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative", overflow: "hidden", cursor: "text", userSelect: "none" }}>
    <text onLayout={(event) => { if (event.width) setCellWidth(event.width / 10); }}
      style={{ position: "absolute", opacity: 0, fontFamily: props.config.fontFamily, fontSize: props.config.fontSize, fontWeight: props.config.fontWeight, pointerEvents: "none" }}>MMMMMMMMMM</text>
    <canvas testId="terminal-viewport" commands={paint()} style={{position:"absolute",left:0,top:offset()-lineHeight,right:0,height:size().height+lineHeight*2,pointerEvents:"none"}}/>
    <Show when={focused() && !surfaceCursorSuppressed(renderer) && frame().cursor.visible && frame().cursor.shape !== "Hidden" && blink() && frame().cursor.y >= 0 && frame().cursor.y < frame().rows}>
      <canvas testId="terminal-cursor-layer" commands={JSON.stringify([{k:"p",c:cssColor(cursorColor()),p:[cursor().quad.flatMap(point=>[point[0],point[1]])]}])}
        style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"none"}}/>
      <div testId="terminal-caret" style={{position:"absolute",left:caret().left,top:caret().top,width:caret().width,height:caret().height,pointerEvents:"none"}}/>
    </Show>
  </div>;
}
