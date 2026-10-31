import type {PresenceAvatar} from "../collaboration/PresenceController";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { createEffect, createMemo, createSignal, Index, onCleanup, Show } from "solid-js";
import { createWindowSize,useGpuixRequired } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import { sharePaintCommands } from "../editor/paint-command";
import { PaintLayer, type PaintCommand } from "./PaintLayer";
import {useOverlayViewport,placeOverlayRect} from "./overlay-viewport";
import {translatePaintCommands} from "./translate-paint";
import {observeTitlebarInset} from "../chrome/window-titlebar";

interface Region { id: string; label: string; rect: number[]; menu: boolean }
interface Frame { commands: PaintCommand[]; regions: Region[]; menuOpen: boolean; menuRect: number[]; animating: boolean; height: number; action?: string | null }
/** Original Rust chrome, including its exact glyph ink centering and menu. */
export function TopBar(props: { config: AppConfig; peers?:readonly PresenceAvatar[]; sidebar?: string; details: boolean; agent: boolean; search: boolean; connection: string;
  menuOpen: boolean; onMenuChange: (open: boolean) => void; onAction: (action: string) => void | Promise<void>; report: (error: unknown) => void }) {
  const size = createWindowSize(), config = props.config;
  const renderer=useGpuixRequired(),viewport=useOverlayViewport(renderer,size);
  const [leftSafeInset,setLeftSafeInset]=createSignal(0);
  onCleanup(observeTitlebarInset(renderer as typeof renderer & {isFullscreen:()=>boolean},process.platform,setLeftSafeInset));
  const id = coreCall<{ id: number }>("chrome.new", { kind: "topbar", family: config.fontFamily, weight: config.fontWeight, fonts: [config.iconFontFamily, "Symbols Nerd Font Mono"] }).id;
  const [frame, setFrame] = createSignal<Frame>({ commands: [], regions: [], menuOpen: false, menuRect: [0, 0, 0, 0], animating: false, height: 30 * config.scale });
  const parameters = createMemo(() => ({ id, width: size().width, height: size().height, scale: config.scale, leftSafeInset:leftSafeInset(), theme: config.themeName,
    sidebar: props.sidebar || null, details: props.details, agent: props.agent, search: props.search, connection: props.connection, peers:props.peers||[], menu: props.menuOpen }), undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let hovered = "";
  const draw = (op = "chrome.frame", args: Record<string, unknown> = {}) => {
    try {
      const started = performance.now(); animation.cancel();
      const next = coreCall<Frame>(op, { ...parameters(), ...args });
      setFrame(previous => ({ ...next, commands: sharePaintCommands(previous.commands, next.commands), regions: next.regions.map((region, index) => {
        const before = previous.regions[index];
        return before && before.id === region.id && before.label === region.label && before.menu === region.menu && before.rect.every((value, i) => value === region.rect[i]) ? before : region;
      }) }));
      if (next.menuOpen !== props.menuOpen) props.onMenuChange(next.menuOpen);
      if (next.action) Promise.resolve(props.onAction(next.action)).catch(props.report);
      if (next.animating) animation.request(() => draw(), started);
    } catch (error) { props.report(error); }
  };
  createEffect(() => { parameters(); draw(); });
  onCleanup(() => { animation.dispose(); coreCall("chrome.dispose", { id }); });
  const height=createMemo(()=>Math.max(frame().height,leftSafeInset()>0?30:0));
  const base = createMemo(() => frame().commands.filter((command) => command.layer < 2));
  const menuRect=createMemo(()=>placeOverlayRect(frame().menuRect,viewport()));
  const menuShift=createMemo(()=>({x:menuRect()[0]-frame().menuRect[0],y:menuRect()[1]-frame().menuRect[1]}));
  const overlay = createMemo(() => translatePaintCommands(frame().commands.filter((command) => command.layer >= 2),frame().menuOpen?menuShift().x:0,frame().menuOpen?menuShift().y:0));
  const move = (x = -1, y = -1) => {
    const hit = frame().regions.find((region) => x >= region.rect[0] && x <= region.rect[0] + region.rect[2] && y >= region.rect[1] && y <= region.rect[1] + region.rect[3]);
    const key = hit?.id || ""; if (key === hovered) return; hovered = key; draw("chrome.pointer", { x, y });
  };
  const hit = (region: () => Region, offset = () => [0, 0]) => <div role="button" aria-label={region().label} testId={region().id}
    onMouseMove={(event) => move((event.x??-1)-(region().menu?menuShift().x:0), (event.y??-1)-(region().menu?menuShift().y:0))} onMouseLeave={() => move()}
    onMouseDown={(event) => { if (event.button === undefined || event.button === 0) { draw("chrome.click", { x: region().rect[0] + region().rect[2] / 2, y: region().rect[1] + region().rect[3] / 2 }); } }}
    style={{ position: "absolute", left: region().rect[0] - offset()[0], top: region().rect[1] - offset()[1], width: region().rect[2], height: region().rect[3], cursor: "pointer", backgroundColor: "transparent" }} />;
  return <div testId="original-topbar" style={{ position: "relative", height: height(), minHeight: height(), flexShrink: 0 }}>
    <PaintLayer commands={base()} config={config} />
    <Index each={frame().regions.filter((region) => !region.menu)}>{(region) => hit(region)}</Index>
    <Show when={overlay().length>0}><anchored fill="window" deferred priority={3} occlude={false} style={{ pointerEvents: "none", backgroundColor: "transparent" }}>
      <PaintLayer commands={overlay()} config={config} />
      <Show when={frame().menuOpen}><div testId="original-hamburger-menu" onMouseDownOutside={(event) => {
        const button = frame().regions.find((region) => region.id === "topbar-menu")?.rect;
        if (button && (event.x || 0) >= button[0] && (event.x || 0) <= button[0] + button[2] && (event.y || 0) >= button[1] && (event.y || 0) <= button[1] + button[3]) return;
        draw("chrome.click", { x: -10000, y: -10000 });
      }}
        style={{ position: "absolute", left: menuRect()[0], top: menuRect()[1], width: menuRect()[2], height: menuRect()[3], pointerEvents: "auto", backgroundColor: "transparent" }}>
        <Index each={frame().regions.filter((region) => region.menu)}>{(region) => hit(region, () => frame().menuRect)}</Index>
      </div></Show>
    </anchored></Show>
  </div>;
}
