import {handleApplicationShortcut} from "../commands/application-shortcuts";
import { createEffect,createMemo, createSignal, For, Index, Show } from "solid-js";
import { createWindowSize, useGpuixRequired, type GpuixTheme, type StyleDesc,type HostElement } from "@gpuix/solid";
import { writeClipboard } from "../clipboard";
import type { AppConfig } from "../config";
import type { EditorLspController, LspMenu, ReferenceRow } from "./lsp";
import {PaintLayer} from "../ui/PaintLayer";
import {focusNativeElement} from "../native-focus";
import {useOverlayViewport} from "../ui/overlay-viewport";
export type { CodeTextEdit, EditorLspConnection } from "./lsp";
export function LspOverlay(props: { controller: EditorLspController; config: AppConfig; anchor: { x: number; y: number; width: number; height: number } }) {
  const s = props.config.scale, c = props.config.theme, window = createWindowSize(), renderer = useGpuixRequired();
  const area=useOverlayViewport(renderer,window);
  const padding=()=>Math.min(8*s,area().width/4,area().height/4);
  const panelWidth=(width:number)=>Math.max(0,Math.min(width,area().width-2*padding()));
  const fitX=(x:number,width:number)=>Math.max(area().x+padding(),Math.min(x,area().x+area().width-padding()-width));
  const fitY=(y:number,height:number)=>Math.max(area().y+padding(),Math.min(y,area().y+area().height-padding()-height));
  const text = (size = 13, color = c.fg): StyleDesc => ({ color, fontFamily: props.config.fontFamily, fontWeight: props.config.fontWeight, fontSize: size * s });
  const theme: GpuixTheme = { bg: c.bg, text: c.fg, textMuted: c.dim, border: c.border, codeText: c.fg, codeWash: c.surface,
    accent: c.accent, fontSans: props.config.fontFamily, fontMono: props.config.fontFamily, metrics: { mdTextSize: 13 * s, mdLineHeight: 19 * s } };
  const menu = () => props.controller.state()?.menu;
  const hover = () => props.controller.state()?.hover;
  let hoverHost:HostElement|undefined;
  createEffect(()=>props.controller.configureHover(props.config,area()));
  const selected = () => { const m = menu(); return m?.items[m.selected ?? 0]; };
  const menuRect=createMemo(()=>{const rect=menu()?.rect;if(!rect)return[0,0,0,0];const width=panelWidth(rect[2]!),height=Math.min(rect[3]!,area().height-2*padding());return[fitX(rect[0]!,width),fitY(rect[1]!,height),width,height];});
  function Rename(input: { initial: string }) {
    const [name, setName] = createSignal(input.initial);
    return <div role="dialog" aria-label="Rename symbol" testId="lsp-rename" style={{ position: "absolute", left: fitX(props.anchor.x+24*s,panelWidth(420*s)), top:fitY(props.anchor.y+24*s,Math.min(110*s,area().height-2*padding())), width: panelWidth(420*s),maxHeight:area().height-2*padding(),overflowY:"scroll",backgroundColor: c.bg,
      borderWidth: s, borderColor: c.border, borderRadius: 8 * s, padding: 12 * s, pointerEvents: "auto" }}>
      <text style={{ ...text(12, c.muted), marginBottom: 8 * s, pointerEvents: "none" }}>Rename symbol</text>
      <input testId="lsp-rename-input" autoFocus={true} value={name()} onChange={(event) => setName(event.value || "")}
        onSubmit={() => props.controller.submitRename(name())} onKeyDown={(event) => { if(handleApplicationShortcut(renderer,event))return;event.stopPropagation(); if (event.key === "escape") { event.preventDefault(); event.stopPropagation(); props.controller.dismiss(); } }}
        style={{ ...text(), width: "100%", height: 28 * s, backgroundColor: c.surface, borderWidth: s, borderColor: c.border }} />
    </div>;
  }
  function References() {
    const [query, setQuery] = createSignal("");
    const rows = createMemo(() => (props.controller.references()?.rows || []).filter((r) => `${r.path} ${r.text}`.toLowerCase().includes(query().toLowerCase())));
    const [selection, setSelection] = createSignal(0);
    const open = (row?: ReferenceRow) => { if (row) props.controller.openReference(row); };
    return <div role="dialog" aria-label="References" testId="lsp-references" onMouseDownOutside={() => props.controller.dismiss()}
      style={{ position: "absolute", top: area().y+padding(), left:area().x+(area().width-panelWidth(700*s))/2, width:panelWidth(700*s), maxHeight:area().height-2*padding(),overflow:"hidden",
        backgroundColor: c.bg, borderWidth: s, borderColor: c.border, borderRadius: 8 * s, padding: 10 * s, pointerEvents: "auto" }}>
      <input testId="lsp-reference-filter" aria-label="Search references" autoFocus={true} value={query()} placeholder="References" onChange={(event) => { setQuery(event.value || ""); setSelection(0); }}
        onSubmit={() => open(rows()[selection()])} onKeyDown={(event) => {
          if(handleApplicationShortcut(renderer,event))return;
          event.stopPropagation();
          if (event.key === "escape") { event.preventDefault(); event.stopPropagation(); props.controller.dismiss(); }
          if (event.key === "up" || event.key === "down") { event.preventDefault(); event.stopPropagation(); setSelection((n) => Math.max(0, Math.min(rows().length - 1, n + (event.key === "up" ? -1 : 1)))); }
        }} style={{ ...text(), width: "100%", height: 30 * s, backgroundColor: c.surface }} />
      <div style={{ maxHeight: Math.max(0,area().height-2*padding()-60*s), overflowY: "scroll" }}><For each={rows()}>{(row, index) => <div role="option" aria-selected={selection() === index()} onClick={() => open(row)}
        style={{ padding: 8 * s, backgroundColor: selection() === index() ? c.hover : undefined, cursor: "pointer", hover: { backgroundColor: c.hover } }}>
        <text style={{ ...text(12, c.blue), pointerEvents: "none" }}>{`${row.path}:${row.line}:${row.column}`}</text>
        <text style={{ ...text(12), whiteSpace: "nowrap", textOverflow: "ellipsis", pointerEvents: "none" }}>{row.text}</text>
      </div>}</For></div>
    </div>;
  }
  return <anchored fill="window" deferred priority={3} occlude={false} onKeyDown={(event) => {
    if(handleApplicationShortcut(renderer,event))return;
    if ((event.modifiers?.ctrl || event.modifiers?.cmd) && event.key?.toLowerCase() === "c") {
      const selected = hover()?.selectedText||renderer.getSelectedText?.();
      if (selected) { event.preventDefault(); event.stopPropagation(); void writeClipboard(selected).catch(props.controller.report); }
    } else if (event.key === "escape") { event.preventDefault(); event.stopPropagation(); props.controller.dismiss(); }
  }} style={{ backgroundColor: "transparent", pointerEvents: "none" }}>
    <Show when={menu()}>{(m) => <div testId="lsp-menu" role="listbox" aria-label={m().kind === "actions" ? "Code actions" : "Completions"}
      onMouseDownOutside={() => props.controller.action("dismiss")}
      onScroll={(event) => props.controller.wheel(event.deltaY || 0, !!event.precise)}
      style={{ position: "absolute", left: menuRect()[0], top: menuRect()[1], width: menuRect()[2], height: menuRect()[3], borderRadius: 8 * s, borderWidth: s, borderColor: c.border,
        backgroundColor: c.bg, overflow: "hidden", pointerEvents: "auto", userSelect: "none" }}>
      <Index each={m().items.slice(Math.max(0,m().first-1),m().first+m().visibleRows+2)}>{(item, relative) => {
        const index = () => Math.max(0,m().first-1)+relative;
        return <div testId={`lsp-option:${item().word}`} role="option" aria-selected={m().selected === index()} onMouseDown={() => props.controller.select(index())}
          style={{ position: "absolute", left: 0, right: 0, top: (index()-m().first)*m().rowHeight+m().offset, height: m().rowHeight,
            paddingLeft: m().padX, paddingRight: m().padX, backgroundColor: m().selected === index() ? c.hover : undefined, cursor: "pointer", hover: { backgroundColor: c.hover } }}>
          <text style={{ ...text(13), position: "absolute", left: m().padX, right: m().kindWidth+m().menuWidth+2*m().padX, top: 4 * s, whiteSpace: "nowrap", textOverflow: "ellipsis", pointerEvents: "none" }}>{item().word}</text>
          <text style={{ ...text(11, c.syn_type), position: "absolute", right: m().menuWidth+m().padX, width: m().kindWidth, top: 5 * s, whiteSpace: "nowrap", textOverflow: "ellipsis", pointerEvents: "none" }}>{item().kind}</text>
          <text style={{ ...text(11,c.muted), position: "absolute", right: m().padX, width: m().menuWidth, top: 5 * s, whiteSpace: "nowrap", textOverflow: "ellipsis", pointerEvents: "none" }}>{item().menu}</text>
        </div>;
      }}</Index>
    </div>}</Show>
    <Show when={menu() && (selected()?.info || selected()?.menu)}><div testId="lsp-completion-docs" style={{ position: "absolute", left:fitX(menuRect()[0]!+menuRect()[2]!+6*s,panelWidth(360*s)),
      top:fitY(menuRect()[1]!+menuRect()[3]!+6*s,Math.min(200*s,area().height-2*padding())), width:panelWidth(360*s), maxHeight:Math.min(200*s,area().height-2*padding()),
      overflowY: "scroll", backgroundColor: c.bg, padding: 12*s, borderWidth: s, borderColor: c.border, borderRadius: 8*s, pointerEvents: "auto" }}>
      <markdown source={[selected()?.word,selected()?.menu,selected()?.info].filter(Boolean).join("\n\n")} theme={theme} />
    </div></Show>
    <Show when={!menu()?hover():undefined}>{(_card)=> <>
      <div testId="lsp-hover-paint" style={{position:"absolute",left:hover()!.paintOrigin[0],top:hover()!.paintOrigin[1],width:hover()!.paintSize[0],height:hover()!.paintSize[1],overflow:"hidden",pointerEvents:"none"}}><PaintLayer commands={hover()!.commands} config={props.config}/></div>
      <div ref={value=>hoverHost=value} tabIndex={0} testId="lsp-hover" role="tooltip" aria-label={hover()!.lines.join("\n")}
        onMouseEnter={()=>props.controller.retainPointer()} onMouseLeave={()=>{if(!hover()?.dragging)props.controller.pointer();}}
        onMouseDown={event=>{if(event.button!==undefined&&event.button!==0)return;props.controller.hoverInput("down",event.x,event.y);if(hoverHost)focusNativeElement(renderer,hoverHost.id);}}
        onMouseMove={event=>props.controller.hoverInput("move",event.x,event.y)}
        onMouseUp={event=>props.controller.hoverInput("up",event.x,event.y)}
        onScroll={event=>props.controller.hoverInput("wheel",event.x,event.y,-(event.deltaY||0)*(event.precise?1:34))}
        style={{position:"absolute",left:hover()!.rect[0],top:hover()!.rect[1],width:hover()!.rect[2],height:hover()!.rect[3],backgroundColor:"transparent",pointerEvents:"auto",userSelect:"none",cursor:hover()?.hoveredLink?"pointer":"text"}}/>
      <Show when={hover()?.dragging}><anchored fill="window" deferred priority={4} occlude={true} style={{backgroundColor:"transparent"}}>
        <div testId="lsp-hover-selection-capture" onMouseMove={event=>props.controller.hoverInput("move",event.x,event.y)} onMouseUp={event=>props.controller.hoverInput("up",event.x,event.y)}
          onScroll={event=>props.controller.hoverInput("wheel",event.x,event.y,-(event.deltaY||0)*(event.precise?1:34))}
          style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",backgroundColor:"transparent",pointerEvents:"auto",cursor:"text"}}/>
      </anchored></Show>
    </>}</Show>
    <Show when={props.controller.rename() !== undefined}><Rename initial={props.controller.rename() || ""} /></Show>
    <Show when={props.controller.references()}><References /></Show>
    <Show when={props.controller.notice()}><div role="status" testId="lsp-notice" style={{ position: "absolute", bottom:window().height-area().y-area().height+padding(), right:window().width-area().x-area().width+padding(), maxWidth:panelWidth(600*s),maxHeight:area().height-2*padding(),overflowY:"scroll",padding: 12*s,
      backgroundColor: c.surface, borderWidth: s, borderColor: props.controller.notice()?.error ? c.red : c.border, borderRadius: 6*s, pointerEvents: "auto" }}>
      <text style={{ ...text(12,props.controller.notice()?.error ? c.red : c.fg), whiteSpace: "normal" }}>{props.controller.notice()?.message}</text>
    </div></Show>
  </anchored>;
}
