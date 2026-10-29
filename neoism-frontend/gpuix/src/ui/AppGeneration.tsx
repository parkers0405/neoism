import { createContext, createEffect, createSignal, For, onCleanup, Show, untrack, type Accessor, type JSX } from "solid-js";
import { useGpuix } from "@gpuix/solid";

/** True for the visible generation while its replacement settles beneath it.
 * Views that drive shared model state (pane layout) freeze instead of
 * re-reading geometry the replacement computes at its own scale. */
export const GenerationRetiring = createContext<Accessor<boolean>>(() => false);

/** Keep the native window root installed while rebuilding scale-dependent
 * components. A replacement generation mounts invisibly beneath the current
 * one and is swapped in only once its layout has settled: entrance motion,
 * measured sizes and async layout feedback all finish off-screen, so a font
 * zoom or reload never shows half-built frames (the desktop never rebuilds). */
export function AppGeneration<T extends object>(props: {
  value: T;
  background?:{path:string;opacity:number};
  backgroundColor?:string;
  children: (value: T) => JSX.Element;
}) {
  const renderer = useGpuix()?.renderer as { getRenderedFrameCount?: () => number } | undefined;
  const [generations, setGenerations] = createSignal<T[]>([untrack(() => props.value)]);
  // The outgoing glyphs and their background are one visual generation.
  // Applying the incoming background first can make light-theme dark ink
  // disappear against a new dark background while its replacement settles.
  const backgrounds = new WeakMap<T, {image?:{path:string;opacity:number};color?:string}>();
  backgrounds.set(untrack(() => props.value), untrack(() => ({image:props.background,color:props.backgroundColor})));
  const background = () => backgrounds.get(generations()[0]);
  let watch: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(watch));
  const promote = (next: T) => { clearTimeout(watch); watch = undefined; setGenerations(list => list.includes(next) ? [next] : list); };
  createEffect(() => {
    const next = props.value;
    backgrounds.set(next,{image:props.background,color:props.backgroundColor});
    if (untrack(generations).includes(next)) return;
    clearTimeout(watch);
    // A newer request supersedes a still-settling one; the visible generation stays.
    setGenerations(list => [list[0], next]);
    // Without a native frame counter (headless hosts) swap atomically.
    const count = renderer?.getRenderedFrameCount?.bind(renderer);
    if (!count) { promote(next); return; }
    // GPUIX draws on demand. The replacement's layout feedback (measured
    // text, onLayout-sized views) settles within a few presented frames: swap
    // at the first idle gap, or after SETTLE_FRAMES when something animates
    // continuously (the Welcome wordmark). Read from the lock-free frame
    // counter so nothing waits on the UI thread; capped if frames stall.
    const started = performance.now(), first = count();
    let seen = first, changedAt = started;
    const check = () => {
      const now = performance.now(), drawn = count(), frames = drawn - first;
      if (drawn !== seen) { seen = drawn; changedAt = now; }
      if (frames >= SETTLE_FRAMES || (frames >= MIN_FRAMES && now - changedAt >= IDLE_MS) || now - started >= 120) promote(next);
      else watch = setTimeout(check, 2);
    };
    watch = setTimeout(check, 2);
  });
  return <div testId="app-shell" style={{
    display: "flex", flexDirection: "column", width: "100%", height: "100%",
    minWidth: 0, minHeight: 0, position: "relative", overflow: "hidden",backgroundColor:background()?.color,
  }}><Show when={background()?.image?.path} keyed>{path=><img testId="app-wallpaper" src={path} objectFit="cover" alt="" style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",opacity:background()?.image?.opacity,pointerEvents:"none"}}/>}</Show>
    <For each={generations()}>{(value) => {
      const pending = () => generations()[0] !== value;
      const retiring = () => !pending() && generations().length > 1;
      return <div testId={pending() ? "app-generation-pending" : "app-generation"} style={{
        position: "absolute", left: 0, top: 0, width: "100%", height: "100%", display: "flex", flexDirection: "column",
        minWidth: 0, minHeight: 0, opacity: pending() ? 0 : 1, pointerEvents: pending() ? "none" : undefined,
      }}><GenerationRetiring.Provider value={retiring}>{props.children(value)}</GenerationRetiring.Provider></div>;
    }}</For></div>;
}

/** A replacement renders at least MIN_FRAMES hidden and is shown at the first
 * idle gap of IDLE_MS (two 144Hz intervals), or after SETTLE_FRAMES. */
const MIN_FRAMES = 2, IDLE_MS = 16, SETTLE_FRAMES = 6;
