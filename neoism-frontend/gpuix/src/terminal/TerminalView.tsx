import { createEffect, createMemo, createSignal, Index, onCleanup, onMount, Show } from "solid-js";
import { useGpuixRequired, type HostElement, type KeyEvent, type EventPayload } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { DaemonConnection } from "../daemon/client";
import { terminalSession } from "./sessions";
import {TerminalCursorAnimation} from "./cursor-animation";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import type {TerminalCursorFrame} from "./buffer";
import {cssColor} from "../ui/canvas";
import { readClipboard, writeClipboard } from "../clipboard";
import { coreCall, type SearchHost } from "../native-core";
import { cursorPaint, sameBounds } from "../editor/paint-frame";
import { focusedNativeElement, focusNativeElement, invalidateNativeFocus } from "../native-focus";
import {CursorBlinkClock} from "../ui/cursor-blink";

export function TerminalView(props: { onCustomKey?: (event: KeyEvent) => boolean; active?: boolean; focusToken?: number; onFocus?: () => void; sessionId: string; client: DaemonConnection; config: AppConfig; onSearch: (kind: "/" | "?", host: SearchHost) => void; report: (error: unknown) => void }) {
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
  const caret = createMemo(() => cursorPaint(cursor().quad));
  let host: HostElement | undefined, selecting = false;
  const send = session.send;
  const cursorAnimation=new TerminalCursorAnimation(()=>buffer.cursor(cellWidth(),lineHeight),setCursor,focused);
  createEffect(()=>{const position=frame().cursor;blinkClock.update({active:focused(),enabled:props.config.cursorBlink,intervalMs:props.config.cursorBlinkInterval,animating:cursor().animating,mode:"terminal",activityToken:`${position.x}:${position.y}:${position.shape}:${cellWidth()}:${lineHeight}`});});
  const receiveFocus = (value: boolean) => {
    if (focused() === value) return;
    setFocused(value); buffer.focus(value);
    if (value) { props.onFocus?.(); cursorAnimation.request(); }
    else { selecting = false; cursorAnimation.cancel(); }
  };
  const cleanupBuffer = buffer.subscribe(() => {setFrame(buffer.frame());cursorAnimation.request();});
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
  onCleanup(() => { cursorAnimation.dispose(); blinkClock.dispose(); buffer.focus(false); cleanupBuffer(); clearInterval(blinkTimer); });
  const intercepted = new Set<string>();
  const keyDown = (event: KeyEvent, released = false) => {
    const key = event.key?.toLowerCase() || "";
    if (released && intercepted.delete(key)) { event.preventDefault(); event.stopPropagation(); return; }
    if (!released && props.onCustomKey?.(event)) { intercepted.add(key); event.preventDefault(); event.stopPropagation(); return; }
    const primary = event.modifiers?.ctrl || event.modifiers?.cmd;
    if (primary && (key === "v" && event.modifiers?.shift || key === "v" && event.modifiers?.cmd)) {
      event.preventDefault(); event.stopPropagation();
      if (!released) void readClipboard().then((text) => {blinkClock.activity();send(buffer.paste(text));}).catch(props.report);
      return;
    }
    if (primary && key === "c" && (event.modifiers?.shift || event.modifiers?.cmd || buffer.selectedText && !buffer.viMode)) {
      event.preventDefault(); event.stopPropagation();
      if (!released && buffer.selectedText) void writeClipboard(buffer.selectedText).catch(props.report);
      return;
    }
    // Leave application shortcuts to the parent.
    if (event.modifiers?.ctrl && (key === "tab" || event.modifiers.shift && ["r", "d", "[", "]", "left", "right", "up", "down", "w"].includes(key))) return;
    if (event.modifiers?.alt && ["e", "n", "c", "h"].includes(key)) return;
    if (event.modifiers?.ctrl && ["`", "p", "t"].includes(key)) return;
    const char = event.keyChar || (key.length === 1 ? key : "");
    if (buffer.viMode && !primary && ["/", "?"].includes(char)) {
      event.preventDefault(); event.stopPropagation();
      if (!released) {
        buffer.search("start", { forward: char === "/" });
        props.onSearch(char as "/" | "?", {
          preview: (pattern) => buffer.search("preview", { pattern }), commit: () => buffer.search("commit"), cancel: () => buffer.search("cancel"),
          focus: () => { if (host) focusNativeElement(renderer, host.id); },
        });
      }
      return;
    }
    const bytes = buffer.key({ key, char: event.keyChar || (key.length === 1 ? key : ""), ...event.modifiers, repeat: event.isHeld, released });
    if (buffer.pendingClipboard !== undefined) { void writeClipboard(buffer.pendingClipboard).catch(props.report); buffer.pendingClipboard = undefined; }
    if (buffer.lastKeyHandled) { event.preventDefault(); event.stopPropagation(); if(!released)blinkClock.activity(); }
    if (bytes.length) { event.preventDefault(); event.stopPropagation(); send(bytes); if (!released) {blinkClock.activity();buffer.scrollToBottom();} }
  };
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
    onMouseDown={(event) => { if (mouse(event, "down")) return; if (event.button === 0) { selecting = true; select(event.x || 0, event.y || 0, true, event.clickCount); } }}
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
    <Index each={frame().lines}>{(line, y) => <div style={{ position: "absolute", left: 0, top: y * lineHeight + offset(), height: lineHeight, width: "100%", pointerEvents: "none" }}>
      <Index each={line()}>{(run) => <text style={{ position: "absolute", left: run().column * cellWidth(), width: run().width * cellWidth(), height: lineHeight,
        fontFamily: props.config.fontFamily, fontSize: props.config.fontSize, fontWeight: run().bold ? 700 : props.config.fontWeight,
        color: run().fg, backgroundColor: run().bg, whiteSpace: "nowrap", lineHeight,
        pointerEvents: "none", textDecoration: run().underline ? "underline" : "none" }}>{run().text}</text>}</Index>
    </div>}</Index>
    <Show when={frame().extra} keyed>{(extra) => <div style={{ position: "absolute", left: 0, top: (extra.above ? -1 : frame().lines.length) * lineHeight + offset(), height: lineHeight, width: "100%", pointerEvents: "none" }}>
      <Index each={extra.line}>{(run) => <text style={{ position: "absolute", left: run().column * cellWidth(), width: run().width * cellWidth(), height: lineHeight,
        fontFamily: props.config.fontFamily, fontSize: props.config.fontSize, fontWeight: run().bold ? 700 : props.config.fontWeight,
        color: run().fg, backgroundColor: run().bg, whiteSpace: "nowrap", lineHeight,
        pointerEvents: "none", textDecoration: run().underline ? "underline" : "none" }}>{run().text}</text>}</Index>
    </div>}</Show>
    <Show when={focused() && !surfaceCursorSuppressed(renderer) && frame().cursor.visible && frame().cursor.shape !== "Hidden" && blink() && frame().cursor.y >= 0 && frame().cursor.y < frame().rows}>
      <canvas commands={JSON.stringify([{k:"p",c:cssColor(cursorColor()),p:[cursor().quad.flatMap(point=>[point[0],point[1]])]}])}
        style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"none"}}/>
      <div testId="terminal-caret" style={{position:"absolute",left:caret().left,top:caret().top,width:caret().width,height:caret().height,pointerEvents:"none"}}/>
    </Show>
  </div>;
}
