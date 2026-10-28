import {handleApplicationShortcut} from "../commands/application-shortcuts";
import { createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { createWindowSize, useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { PaintLayer } from "../ui/PaintLayer";
import { LoadingSkeleton } from "../ui/LoadingSkeleton";
import { readClipboard } from "../clipboard";
import { ChromeActionsController } from "./ChromeActionsController";
import {useOverlayViewport,overlayPoint} from "../ui/overlay-viewport";

export function ChromeActionsView(props: { controller: ChromeActionsController; config: AppConfig; pageMode?: boolean; active?: boolean; focusToken?: number; onFocus?: () => void; onCustomKey?: (event: KeyEvent) => boolean }) {
  const renderer = useGpuixRequired(), window = createWindowSize(), page = !!props.pageMode;
  const viewport=useOverlayViewport(renderer,window);
  const [frame, setFrame] = createSignal(page ? props.controller.extensionFrame : props.controller.frame);
  const [bounds, setBounds] = createSignal({ x: 0, y: 0, width: 0, height: 0 });
  const imagePath = createMemo(() => frame()?.image?.path);
  let host: HostElement | undefined, previousFocus: number | null | undefined;
  const update = () => setFrame(page ? props.controller.extensionFrame : props.controller.frame);
  onCleanup(props.controller.subscribe(update));
  const focus = () => { props.onFocus?.(); if (host) renderer.focusElement?.(host.id); };
  onMount(() => { previousFocus = renderer.getFocusedElementId?.(); if (page) void props.controller.ensureExtensions(); focus(); });
  onCleanup(() => { if (!page && previousFocus != null) renderer.focusElement?.(previousFocus); });
  createEffect(() => { props.focusToken; if (props.active !== false && host) focus(); });
  createEffect(() => { if (!page) { const size = viewport(); props.controller.layout(size.width, size.height); } });
  const point = (event: { x?: number; y?: number }) => overlayPoint(page?bounds():viewport(),event);
  const key = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return;
    if (page && props.onCustomKey?.(event)) return;
    event.preventDefault(); event.stopPropagation();
    if ((event.modifiers?.ctrl || event.modifiers?.cmd) && event.key?.toLowerCase() === "v") {
      void readClipboard().then((text) => props.controller.call("text", { text }, page)).catch(props.controller.options.model.report); return;
    }
    props.controller.call("key", { key: event.key?.toLowerCase(), char: event.keyChar || (event.key?.length === 1 ? event.key : ""), ...event.modifiers, repeat: event.isHeld }, page);
  };
  const content = <div ref={host} testId={page ? "extensions-page" : "chrome-action-overlay"} role={page ? "region" : "dialog"} aria-label={frame()?.kind || "Extensions"} tabIndex={0}
    onFocus={props.onFocus} onKeyDown={key}
    onLayout={(event) => { const next = { x: event.x || 0, y: event.y || 0, width: event.width || 0, height: event.height || 0 }; setBounds((old) => Object.keys(next).every((key) => next[key as keyof typeof next] === old[key as keyof typeof old]) ? old : next); if (page && next.width && next.height) props.controller.layout(next.width, next.height, true); }}
    onMouseDown={(event) => { if (event.button !== undefined && event.button !== 0) return; focus(); props.controller.call("click", point(event), page); }}
    onMouseMove={(event) => props.controller.call("pointer", point(event), page)}
    onScroll={(event) => props.controller.call("scroll", { ...point(event), delta: (event.deltaY || 0) * (event.precise ? 1 : 40 * props.config.scale) }, page)}
    style={page ? { position: "relative", flexGrow: 1, minHeight: 0, minWidth: 0, overflow: "hidden", backgroundColor: props.config.theme.bg } : { position: "absolute", left: viewport().x, top: viewport().y, width: viewport().width, height: viewport().height, overflow: "hidden" }}>
    <Show when={frame()} fallback={<LoadingSkeleton kind="document" config={props.config} />}>
      <PaintLayer commands={frame()?.commands || []} config={props.config} />
      <Show when={imagePath()} keyed>{(path) => <img src={path} objectFit="contain" alt="Background preview" style={{ position: "absolute", left: frame()?.image?.rect[0], top: frame()?.image?.rect[1], width: frame()?.image?.rect[2], height: frame()?.image?.rect[3], pointerEvents: "none" }} />}</Show>
    </Show>
  </div>;
  return page ? content : <anchored fill="window" deferred priority={3} occlude={true} style={{ backgroundColor: "transparent" }}>{content}</anchored>;
}
