import { createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { createWindowSize, useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import { PaintLayer, type PaintCommand } from "../ui/PaintLayer";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusNativeElement } from "../native-focus";
import {handleApplicationShortcut} from "../commands/application-shortcuts";

export interface NavigatorRow { value: string; title: string; description?: string; footer?: string; current?: boolean; disabled?: boolean }
interface NavigatorFrame { commands: PaintCommand[]; animating: boolean; active: boolean; action?: string | null }

/** The desktop sidebar Projects dropdown (shared `ConversationNavigator`),
 * anchored under the header button and painted by the original renderer. */
export function ConversationNavigatorView(props: { config: AppConfig; rows: NavigatorRow[]; anchor?: number[] | null; onDone: (value: string | undefined) => void }) {
  const config = props.config, renderer = useGpuixRequired(), windowSize = createWindowSize();
  const { id } = coreCall<{ id: number }>("convnav.new", { rows: props.rows, anchor: props.anchor ?? null, theme: config.themeName, scale: config.scale, fontFamily: config.fontFamily, fontWeight: config.fontWeight });
  const [commands, setCommands] = createSignal<readonly PaintCommand[]>([]);
  let size = { width: 0, height: 0 }, host: HostElement | undefined, done = false;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const finish = (value: string | undefined) => { if (done) return; done = true; props.onDone(value); };
  const call = (op: string, args: Record<string, unknown> = {}) => {
    if (done || size.width <= 0) return;
    const started = performance.now(); animation.cancel();
    const next = coreCall<NavigatorFrame>(`convnav.${op}`, { id, width: size.width, height: size.height, ...args });
    setCommands(next.commands);
    if (next.action) { finish(next.action); return; }
    if (!next.active) { finish(undefined); return; }
    if (next.animating) animation.request(() => call("frame"), started);
  };
  createEffect(() => { const next = windowSize(); if (next.width !== size.width || next.height !== size.height) { size = { width: next.width, height: next.height }; call("frame"); } });
  onMount(() => { if (host) focusNativeElement(renderer, host.id); });
  onCleanup(() => { animation.dispose(); coreCall("convnav.dispose", { id }); });
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return;
    event.preventDefault(); event.stopPropagation();
    call("key", { key: event.key || "", char: event.keyChar || "", shift: !!event.modifiers?.shift, ctrl: !!event.modifiers?.ctrl });
  };
  return <anchored fill="window" deferred={true} priority={3} occlude={true} style={{ backgroundColor: "transparent" }}>
    <div ref={host} tabIndex={0} testId="conversation-navigator" aria-label="Projects" onKeyDown={keyDown}
      onMouseDown={(event) => call("pointer", { kind: "down", x: event.x || 0, y: event.y || 0 })}
      onMouseMove={(event) => call("pointer", { kind: "move", x: event.x || 0, y: event.y || 0 })}
      // Desktop: `conversation_navigator.scroll(-dy)` with 34px per notch.
      onScroll={(event) => call("scroll", { delta: -(event.precise ? event.deltaY || 0 : (event.deltaY || 0) * 34) })}
      style={{ position: "absolute", left: 0, top: 0, width: windowSize().width, height: windowSize().height, userSelect: "none" }}>
      <PaintLayer commands={commands()} config={config} />
    </div>
  </anchored>;
}
