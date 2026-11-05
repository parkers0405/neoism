import { createMemo, createSignal } from "solid-js";
import { useGpuixRequired, type EventPayload, type HostElement } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { paintedChromeBounds } from "../ui/chrome-target-bounds";

/** Interactive rail for a native scroll handle; capture stays on the pressed thumb. */
export function DatabaseScrollbar(props: {
  axis: "horizontal" | "vertical";
  viewport: number;
  content: number;
  offset: number;
  config: AppConfig;
  onChange: (offset: number) => void;
  onWheel?: (event:EventPayload)=>void;
}) {
  const renderer = useGpuixRequired();
  const vertical = props.axis === "vertical";
  const scale = () => props.config.scale;
  const maximum = createMemo(() => Math.max(0, props.content - props.viewport));
  const length = createMemo(() => Math.min(props.viewport, Math.max(24 * scale(), props.viewport * props.viewport / Math.max(1, props.content))));
  const travel = createMemo(() => Math.max(0, props.viewport - length()));
  const position = createMemo(() => maximum() ? Math.max(0, Math.min(maximum(), props.offset)) / maximum() * travel() : 0);
  const [dragging, setDragging] = createSignal(false);
  let host: HostElement | undefined;
  let drag: { pointer: number; offset: number } | undefined;
  const coordinate = (event: EventPayload) => vertical ? event.y : event.x;
  const change = (offset: number) => props.onChange(Math.max(0, Math.min(maximum(), offset)));
  const end = () => { drag = undefined; setDragging(false); };
  return <div ref={element => host = element} testId={`database-scrollbar-${props.axis}`} style={{
    position: "relative", flexShrink: 0, flexGrow: vertical ? 0 : 1, flexBasis: vertical ? undefined : 0,
    width: vertical ? 12 * scale() : undefined, height: vertical ? "100%" : 12 * scale(),
    backgroundColor: props.config.theme.surface, minWidth: 0,
  }} onScroll={props.onWheel} onMouseDown={event => {
    if (event.button !== undefined && event.button !== 0 || !host) return;
    const pointer = coordinate(event), bounds = paintedChromeBounds(renderer, host.id);
    if (pointer === undefined || !bounds) return;
    const at = pointer - (vertical ? bounds.y : bounds.x);
    if (at >= position() && at <= position() + length()) return;
    change((at - length() / 2) / Math.max(1, travel()) * maximum());
  }}>
    <div role="scrollbar" aria-label={`${vertical ? "Vertical" : "Horizontal"} table scroll`} aria-orientation={props.axis}
      aria-valuemin={0} aria-valuemax={Math.round(maximum())} aria-valuenow={Math.round(props.offset)} tabIndex={0}
      testId={`database-scrollbar-thumb-${props.axis}`}
      onMouseDown={event => {
        const pointer = coordinate(event);
        if (pointer === undefined || event.button !== undefined && event.button !== 0) return;
        drag = { pointer, offset: props.offset }; setDragging(true);
      }}
      onMouseMove={event => {
        if (!drag) return;
        if (event.pressedButton !== undefined && event.pressedButton !== 0) { end(); return; }
        const pointer = coordinate(event);
        if (pointer !== undefined) change(drag.offset + (pointer - drag.pointer) / Math.max(1, travel()) * maximum());
      }} onMouseUp={end}
      onKeyDown={event => {
        if (event.modifiers?.alt || event.modifiers?.ctrl || event.modifiers?.cmd) return;
        const key = event.key?.toLowerCase().replace(/^arrow/, "");
        if (key === "home") change(0); else if (key === "end") change(maximum());
        else if (key === "pageup" || key === "pagedown") change(props.offset + (key === "pageup" ? -1 : 1) * props.viewport);
        else if (key === (vertical ? "up" : "left") || key === (vertical ? "down" : "right")) change(props.offset + (key === "up" || key === "left" ? -1 : 1) * 40 * scale());
        else return;
        event.preventDefault(); event.stopPropagation();
      }}
      style={{ position: "absolute", left: vertical ? 3 * scale() : position(), top: vertical ? position() : 3 * scale(),
        width: vertical ? 6 * scale() : length(), height: vertical ? length() : 6 * scale(),
        borderRadius: 3 * scale(), backgroundColor: dragging() ? props.config.theme.muted : props.config.theme.border,
        cursor: dragging() ? "grabbing" : "grab", hover: { backgroundColor: props.config.theme.muted } }} />
  </div>;
}