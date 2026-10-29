import { createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { createWindowSize, useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import { readClipboard, writeClipboard } from "../clipboard";
import type { AppConfig } from "../config";
import { coreCall, NativeEditor, type EditorFrame } from "../native-core";
import { createEditorLsp, type EditorLspConnection } from "./lsp";
import { LspOverlay } from "./LspOverlay";
import { BreadcrumbsView } from "./BreadcrumbsView";
import {PaintLayer} from "../ui/PaintLayer";
import { CanvasImages, cssColor, RasterSource, type CanvasOp } from "../ui/canvas";
import { focusedNativeElement, focusNativeElement, invalidateNativeFocus } from "../native-focus";
import {registerSurfaceCaret,surfaceCursorSuppressed} from "../ui/cursor-handoff";
import { cursorPaint, documentPaintRows, sameBounds, type EditorPaintRow } from "./paint-frame";
import { wheelPixels } from "../ui/wheel";
import { CursorBlinkClock } from "../ui/cursor-blink";

export function EditorView(props: {
  onCustomKey?: (event: KeyEvent) => boolean;
  lsp?: EditorLspConnection; onFocus?: () => void; onClose?: () => void;
  active?: boolean; editor: NativeEditor; config: AppConfig; focusToken: number; prompt: (kind: "/" | "?" | ":") => void; save: () => Promise<void>;
  report: (error: unknown) => void; onState: (state: { mode: string; line: number; col: number; dirty: boolean }) => void;
}) {
  const editor = props.editor, config = props.config, c = config.theme;
  const renderer = useGpuixRequired();
  const window = createWindowSize();
  const [frame, setFrame] = createSignal<EditorFrame>(editor.frame);
  const [focused, setFocused] = createSignal(false);
  const active = createMemo(() => props.active !== false);
  const [blink, setBlink] = createSignal(false);
  const [color, setColor] = createSignal(config.cursorColor || c.accent);
  const blinkClock = new CursorBlinkClock(setBlink, { scope: renderer });
  onCleanup(() => blinkClock.dispose());
  createEffect(() => {
    const current = frame();
    blinkClock.update({
      active: focused() && active() && current.cursor.visible && !surfaceCursorSuppressed(renderer),
      enabled: config.cursorBlink, intervalMs: config.cursorBlinkInterval,
      animating: current.animating,
      activityToken: [current.cursor.x, current.cursor.y, current.cursor.shape, current.cellWidth, current.rowHeight].join(":"),
    });
  });
  const [size, setSize] = createSignal({ width: 0, height: 0, x: 0, y: 0 }, { equals: sameBounds });
  const [cell, setCell] = createSignal(editor.frame.cellWidth || config.fontSize * 0.6);
  let host: HostElement | undefined, selecting = false;
  let inputQueue = Promise.resolve();
  let reported: EditorFrame | undefined;
  const display = (next: EditorFrame) => {
    if (next.clipboard != null) void writeClipboard(next.clipboard).catch(props.report);
    setFrame(next);
    if (!reported || reported.mode !== next.mode || reported.cursor.line !== next.cursor.line || reported.cursor.col !== next.cursor.col || reported.dirty !== next.dirty
      || reported.revision !== next.revision || reported.totalLines !== next.totalLines || reported.pendingKeys !== next.pendingKeys
      || reported.lsp?.diagnostics !== next.lsp?.diagnostics) {
      reported = next; props.onState({ mode: next.mode, line: next.cursor.line, col: next.cursor.col, dirty: next.dirty });
    }
    if (next.animating) editor.requestFrame();
  };
  const receiveFocus = (value: boolean) => {
    if (focused() === value) return;
    setFocused(value);
    if (value) { props.onFocus?.(); reported = undefined; display(editor.frame); }
    else {
      if (selecting) { selecting = false; editor.call("pointer-up"); }
    }
  };
  onCleanup(editor.subscribe(display));
  const lsp = props.lsp ? createEditorLsp(editor, props.lsp, { save: props.save, report: props.report,
    focus: () => { if (host) focusNativeElement(renderer, host.id); } }) : undefined;
  onCleanup(() => lsp?.dispose());
  createEffect(() => { if (lsp) lsp.geometry(size(), window(), config.scale); });
  createEffect(() => { editor.call("raster.configure", {fontFamily:config.fontFamily,fontSize:config.fontSize,fontWeight:config.fontWeight,colors:{Plain:c.fg,Keyword:c.syn_keyword,Type:c.syn_type,String:c.syn_string,Number:c.syn_number,Comment:c.syn_comment,Function:c.syn_func,Property:c.syn_property,Constructor:c.syn_constructor,Special:c.syn_special}}); });
  createEffect(() => { props.focusToken; if (active() && host) focusNativeElement(renderer, host.id); });
  createEffect(() => {
    const viewport = size(); if (!viewport.width || !viewport.height) return;
    editor.call("layout", { width: viewport.width, height: viewport.height, cellWidth: cell(), rowHeight: Math.round(config.fontSize * 1.5) });
  });
  const tick = setInterval(() => {
    const owner = focusedNativeElement(renderer);
    if (owner !== undefined && host?.id) receiveFocus(owner === host.id);
    if (!focused()) return;
    if (config.cursorStyle === "rainbow") {
      const style = coreCall<{ rainbow: number[] }>("cursor", { seconds: performance.now() / 1000 });
      setColor("#" + style.rainbow.slice(0, 3).map((n) => n.toString(16).padStart(2, "0")).join(""));
    }
  }, 1000 / 60);
  onCleanup(() => { clearInterval(tick); editor.cancelFrame(); });
  let releaseCaret=()=>{};
  onMount(() => { if(host)releaseCaret=registerSurfaceCaret(renderer,host,()=>frame().cursorQuad.map(point=>[point[0]+size().x,point[1]+size().y]));if (active() && host) focusNativeElement(renderer, host.id); });
  onCleanup(()=>releaseCaret());
  const keyDown = (event: KeyEvent) => {
    if (props.onCustomKey?.(event)) { event.preventDefault(); event.stopPropagation(); return; }
    const key = event.key?.toLowerCase() || "";
    if (lsp) {
      const primary = event.modifiers?.ctrl || event.modifiers?.cmd;
      const action = key === "f12" ? event.modifiers?.shift ? "references" : "definition" : key === "f2" ? "rename"
        : primary && ["space", " "].includes(key) ? event.modifiers?.shift ? "signature" : "completion"
        : primary && key === "." ? "actions" : event.modifiers?.alt && event.modifiers?.shift && key === "f" ? "format" : undefined;
      if (action) { event.preventDefault(); event.stopPropagation(); lsp.action(action); return; }
      if (lsp.state()?.menu && primary && ["n","p"].includes(key)) {
        event.preventDefault(); event.stopPropagation(); editor.call("key", { key, ctrl: true }); return;
      }
    }
    if ((event.modifiers?.ctrl || event.modifiers?.cmd) && key === "s") {
      event.preventDefault(); event.stopPropagation(); void props.save().then(() => display(editor.frame)).catch(props.report); return;
    }
    if (event.modifiers?.ctrl && (key === "tab" || event.modifiers.shift && ["r", "d", "[", "]", "left", "right", "up", "down", "w"].includes(key))) return;
    if (event.modifiers?.alt && !(event.modifiers.ctrl && ["up", "down"].includes(key)) || event.modifiers?.cmd && !["a", "c", "x", "v", "z"].includes(key) || event.modifiers?.ctrl && ["p", "t", "`"].includes(key)) return;
    event.preventDefault(); event.stopPropagation();
    const char = event.keyChar || (key.length === 1 ? key : "");
    const primary = event.modifiers?.ctrl || event.modifiers?.cmd, shift = event.modifiers?.shift;
    inputQueue = inputQueue.then(async () => {
      const modal = (editor.frame.vim ?? config.vim) && editor.frame.mode !== "Insert";
      if (modal && !primary && editor.frame.pendingEmpty && ["/", "?", ":"].includes(char)) {
        props.prompt(char as "/" | "?" | ":"); return;
      }
      if (primary && ["c", "x"].includes(key)) {
        const payload = coreCall<{ text: string }>(`editor.${key === "c" ? "copy" : "cut"}`, { id: editor.id });
        editor.call("frame"); await writeClipboard(payload.text); return;
      }
      if (primary && key === "v" && (!modal || shift)) {
        editor.call("paste", { text: await readClipboard() }); return;
      }
      let clipboard: string | undefined;
      if (modal && !primary && ["p", "P", "."].includes(char)) {
        try { clipboard = await readClipboard(); } catch { /* Internal registers remain usable. */ }
      }
      const args = { key, char, ctrl: primary, alt: event.modifiers?.alt, shift, clipboard };
      const result = !primary && !event.modifiers?.alt && ["up", "down"].includes(key) ? editor.queueKey(args) : editor.call("key", args);
      if (result.hostAction === "close") props.onClose?.();
    }).catch(props.report);
  };
  let paintedRows: EditorPaintRow[] = [];
  const rows = createMemo(() => paintedRows = documentPaintRows(paintedRows, frame()));
  const token = (name: string) => ({ Keyword: c.syn_keyword, Type: c.syn_type, String: c.syn_string, Number: c.syn_number,
    Comment: c.syn_comment, Function: c.syn_func, Property: c.syn_property, Constructor: c.syn_constructor, Special: c.syn_special }[name] || c.fg);
  // The whole viewport is one native display list: per-row hosts cost a GPUI
  // element, layout node and paint pass each on every frame.
  const caretShown = createMemo(() => focused() && !surfaceCursorSuppressed(renderer) && frame().cursor.visible && blink() && frame().cursorQuad.length > 0);
  const caret = createMemo(() => cursorPaint(frame().cursorQuad));
  const images = new CanvasImages(renderer);
  onCleanup(() => images.dispose());
  const paint = createMemo(() => {
    const f = frame(), rh = f.rowHeight, ops: CanvasOp[] = [], wide = 1e5;
    const font = config.fontFamily, size = config.fontSize, weight = config.fontWeight;
    ops.push({ k: "r", b: [0, f.cursor.y, wide, rh], c: cssColor(c.hover + "88") });
    for (const hit of f.searchHits) ops.push({ k: "r", b: [hit.x, hit.y, hit.width, rh], c: cssColor(hit.selection ? c.accent + "47" : c.yellow + "4d") });
    const selection = cssColor(c.blue + "66"), activeLine = f.cursor.line;
    for (const line of rows()) {
      const top = line.y - f.scrollY;
      ops.push({ k: "t", x: 10, y: top, t: line.continuation ? "↳" : String(line.line + 1), s: size, c: cssColor(line.line === activeLine ? c.fg : c.muted),
        f: font, w: weight, lh: rh, aw: Math.max(0, f.gutterWidth - 20), al: "r" });
      for (const run of line.runs) if (run.selected) ops.push({ k: "r", b: [run.x, top, run.width || 0, rh], c: selection });
      if (line.raster !== undefined) {
        const raster = line.raster;
        if (raster) ops.push({ k: "i", id: images.use(RasterSource.editor, raster), b: [raster.x, top + raster.y, raster.width / raster.scale, raster.height / raster.scale] });
      } else for (const run of line.runs) if (run.text) ops.push({ k: "t", x: run.x, y: top, t: run.text, s: size, c: cssColor(token(run.token)), f: font, w: weight, lh: rh });
      for (const run of line.runs) {
        if (!run.severity) continue;
        const width = Math.max(1, run.width || run.text.length * cell());
        const tone = run.severity === "Error" ? c.red : run.severity === "Warn" ? c.yellow : run.severity === "Info" ? c.blue : c.muted;
        // Match the original Code renderer's diagnostic band. Spellcheck and
        // plugin underlines have separate paint paths and keep their styles.
        ops.push({ k: "r", c: cssColor(tone), b: [run.x, top + rh - 3, width, 2] });
      }
    }
    images.commit();
    const caretTone = cssColor(c.accent + (f.mode === "Insert" ? "d9" : "8c"));
    for (const caret of f.extraCarets) ops.push({ k: "r", b: [caret.x, caret.y, caret.width, caret.height], c: caretTone });
    if (caretShown())
      ops.push({ k: "p", c: cssColor(color()), p: [f.cursorQuad.flatMap((point) => [point[0], point[1]])] });
    return JSON.stringify(ops);
  });
  const pointer = (x = 0, y = 0, extend: boolean, clicks = 1, drag = false) => {
    if (!drag) props.onFocus?.();
    if (host) focusNativeElement(renderer, host.id);
    editor.call("pointer", { x: x - size().x, y: y - size().y, extend, clicks, drag });
  };
  return <div style={{display:"flex",flexDirection:"column",flexGrow:1,minHeight:0,minWidth:0}}>
    <BreadcrumbsView editor={editor} frame={frame()} config={config} path={props.lsp?.path} root={props.lsp?.root} report={props.report}/>
    <div ref={host} tabIndex={0} testId="code-editor" aria-label="Code editor" role="textbox"
    onKeyDown={keyDown} onFocus={() => { invalidateNativeFocus(renderer); receiveFocus(true); }} onBlur={() => { invalidateNativeFocus(renderer); receiveFocus(false); }}
    onLayout={(event) => setSize({ x: event.x || 0, y: event.y || 0, width: event.width || 0, height: event.height || 0 })}
    onScroll={(event) => {if(lsp?.hoverCaptures(event.x,event.y))return;editor.queueScroll(wheelPixels(event, config.fontSize).y);}}
    onMouseDown={(event) => { if(lsp?.hoverCaptures(event.x,event.y)||event.button !== 0) return;
      if (lsp && (event.modifiers?.ctrl || event.modifiers?.cmd)) { pointer(event.x,event.y,false); lsp.action("definition"); return; }
      selecting = true; pointer(event.x, event.y, event.modifiers?.shift || false, event.clickCount || 1); }}
    onMouseMove={(event) => { if(lsp?.hoverCaptures(event.x,event.y))return; if (selecting && event.pressedButton === 0) pointer(event.x, event.y, true, 1, true);
      else lsp?.pointer((event.x || 0)-size().x,(event.y || 0)-size().y); }}
    onMouseLeave={() => lsp?.pointer()}
    onMouseUp={event => {if(lsp?.hoverCaptures(event.x,event.y))return; selecting = false; editor.call("pointer-up"); }}
    style={{ flexGrow: 1, minHeight: 0, minWidth: 0, position: "relative", overflow: "hidden", cursor: "text", userSelect: "none" }}>
    <text onLayout={(event) => { if (event.width) setCell(event.width / 10); }} style={{ position: "absolute", opacity: 0,
      fontFamily: config.fontFamily, fontSize: config.fontSize, fontWeight: config.fontWeight, pointerEvents: "none" }}>0000000000</text>
    <canvas testId="code-viewport" commands={paint()} style={{ position: "absolute", left: 0, top: 0, right: 0, bottom: 0, pointerEvents: "none" }} />
    <PaintLayer commands={frame().peerCommands||[]} config={config}/>
    {/* Painted by the canvas; this empty box only locates the caret for automation. */}
    <Show when={caretShown()}><div testId="code-editor-caret" style={{ position: "absolute", left: caret().left, top: caret().top, width: caret().width, height: caret().height, pointerEvents: "none" }} /></Show>
    <Show when={lsp}><LspOverlay controller={lsp!} config={config} anchor={size()} /></Show>
    </div>
  </div>;
}
