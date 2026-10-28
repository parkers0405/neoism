import {handleApplicationShortcut} from "./application-shortcuts";
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { createWindowSize, useGpuixRequired, type HostElement, type KeyEvent, type StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { CommandController, type CommandTarget } from "./CommandController";
import {centeredOverlayOffset,useOverlayViewport} from "../ui/overlay-viewport";

export function CommandView(props: { controller: CommandController; config: AppConfig; onAccept: (target: CommandTarget) => void; onClose: () => void }) {
  const config = props.config, c = config.theme, s = config.scale, renderer = useGpuixRequired(), size = createWindowSize();
  const viewport=useOverlayViewport(renderer,size);
  const [frame, setFrame] = createSignal(props.controller.frame);
  createEffect(() => { const controller = props.controller; setFrame(controller.frame); onCleanup(controller.subscribe(setFrame)); });
  createEffect(() => { props.controller.layout(viewport().width, viewport().height); });
  let input: HostElement | undefined;
  onMount(() => { if (input) renderer.focusElement?.(input.id); });
  const accept = (index?: number) => { if (index !== undefined) props.controller.select(index); const target = props.controller.target(); if (target) props.onAccept(target); };
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return true;
    event.stopPropagation();
    if (event.key === "escape") { event.preventDefault(); props.onClose(); }
    else if (event.key === "up" || event.key === "down") { event.preventDefault(); props.controller.move(event.key === "up" ? -1 : 1); }
  };
  const text = (size = frame().fontSize, color = c.fg): StyleDesc => ({ color, fontFamily: config.fontFamily, fontSize: size,
    fontWeight: config.fontWeight, whiteSpace: "nowrap", pointerEvents: "none", userSelect: "none", textOverflow: "ellipsis" });
  const position = (rect: number[]): StyleDesc => ({ position: "absolute", left: rect[0], top: rect[1], width: rect[2], height: rect[3] });
  const rows = () => frame().rows.slice(Math.max(0, frame().top - 3), frame().top + frame().visibleRows + 4);
  const dialogPosition=()=>{const rect=frame().rect,offset=centeredOverlayOffset(viewport(),rect);return position([rect[0]+offset.x,rect[1]+offset.y,rect[2],rect[3]]);};
  return <anchored fill="window" deferred={true} priority={2} occlude={false} style={{ backgroundColor: "transparent", pointerEvents: "none" }}>
    <div style={{position:"absolute",left:viewport().x,top:viewport().y,width:viewport().width,height:viewport().height,overflow:"hidden",pointerEvents:"none"}}><div testId="command-dialog" role="dialog" aria-label="Command palette" onKeyDown={keyDown} onMouseDownOutside={props.onClose}
      style={{ ...dialogPosition(), backgroundColor: c.bg, borderTopLeftRadius: frame().radius, borderTopRightRadius: frame().radius, overflow: "hidden", pointerEvents: "auto" }}>
      <div style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", borderWidth: frame().stroke, borderColor: c.surface,
        borderTopLeftRadius: frame().radius, borderTopRightRadius: frame().radius, pointerEvents: "none" }} />
      <input ref={input} testId="command-palette" aria-label="Type a command" autoFocus={true} value={frame().query} placeholder="Type a command..."
        onChange={(event) => props.controller.query(event.value || "")} onSubmit={() => accept()}
        style={{ ...text(frame().inputFontSize), ...position(frame().inputRect), backgroundColor: c.bg, borderWidth: 0,
          paddingLeft: 10 * s, paddingRight: 10 * s, pointerEvents: "auto", userSelect: "text" }} />
      <div testId="command-results" onScroll={(event) => props.controller.scroll(event.precise ? event.deltaY || 0 : (event.deltaY || 0) * 22 * 3)}
        style={{ ...position(frame().listRect), overflow: "hidden" }}>
        <Show when={frame().rows.length}><div style={{ position: "absolute", left: 0, top: frame().selectedY, width: "100%", height: frame().rowHeight, backgroundColor: c.hover, borderRadius: 3 * s, pointerEvents: "none" }} /></Show>
        <For each={rows().map((r) => r.index)}>{(index) => {
          const initial = frame().rows[index]; const row = () => frame().rows[index] || initial;
          return <div role="option" testId={`command:${row().action || row().exCommand || index}`} aria-selected={frame().selected === index} aria-label={row().displayTitle}
            onMouseDown={() => accept(index)} onMouseMove={() => { if (frame().selected !== index) props.controller.select(index); }}
            style={{ position: "absolute", left: 0, top: (index - frame().top) * frame().rowHeight + frame().offset, width: "100%", height: frame().rowHeight,
              display: "flex", alignItems: "center", paddingLeft: 10 * s, paddingRight: 10 * s, gap: 8 * s, overflow: "hidden", cursor: "pointer" }}>
            <text style={{ ...text(frame().fontSize, c.dim), fontFamily: config.iconFontFamily, flexShrink: 0 }}>{row().glyph}</text>
            <text style={{ ...text(), flexShrink: 1, flexGrow: 1 }}>{row().displayTitle}</text>
            <text style={{ ...text(frame().shortcutFontSize, c.muted), flexShrink: 1 }}>{row().shortcut}</text>
          </div>;
        }}</For>
      </div>
    </div></div>
  </anchored>;
}
