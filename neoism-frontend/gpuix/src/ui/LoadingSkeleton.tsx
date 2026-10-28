import { createMemo, createSignal, Index, onCleanup, onMount } from "solid-js";
import type { StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { easeOutCubic } from "../design/animation";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";

export type SkeletonKind = "tree" | "sessions" | "picker" | "document";
export interface SkeletonBar { x: number; y: number; width: number; height: number; radius: number; alpha: number }
const widths = [0.58, 0.72, 0.46, 0.64, 0.38, 0.55, 0.68, 0.44, 0.52, 0.36, 0.62, 0.48];
const depths = [0, 0, 1, 1, 2, 1, 0, 1, 2, 2, 0, 1];

/** Geometry and clocks from file_tree/render.rs, side_panel/draw.rs,
 * widgets/inline_picker.rs and markdown/render/virtualized/surface.rs. */
export function loadingSkeletonBars(kind: SkeletonKind, width: number, height: number, scale: number, elapsed: number): SkeletonBar[] {
  const s = Math.max(0.5, Math.min(3, scale)), bars: SkeletonBar[] = [];
  const add = (x: number, y: number, width: number, height: number, radius: number, alpha: number) => {
    if (width > 1 && height > 0) bars.push({ x, y, width, height, radius, alpha });
  };
  const fade = Math.min(1, Math.max(0, elapsed) / 0.18);
  const alpha = (i: number) => Math.max(0.04, 0.16 + 0.08 * Math.sin(elapsed / 1.3 * Math.PI * 2 - i * 0.55)) * fade;
  if (kind === "picker") {
    const fractions = [[0.42, 0.68], [0.58, 0.46], [0.36, 0.60]], row = 46 * s, available = Math.max(0, width - 44 * s);
    fractions.forEach(([title, description], i) => {
      const y = i * row; if (y + row > height + 0.5) return;
      add(22 * s, y + 8 * s, available * title, 12 * s, 6 * s, alpha(i));
      add(22 * s, y + 26 * s, available * description, 9 * s, 4.5 * s, alpha(i) * 0.7);
    });
  } else if (kind === "document") {
    const ramp = easeOutCubic((elapsed * 1000 - 80) / 240), row = 16 * s * (14 / 17) * 1.3;
    const contentWidth = Math.max(220, Math.min(920, width - 96)), x = Math.max((width - contentWidth) * 0.5, Math.min(48, width * 0.08));
    const factors = [0.72, 0.55, 0.85, 0.64];
    for (let i = 0, y = 22; y <= height; i++, y += row) {
      add(x, y, contentWidth * factors[i % 4], row * 0.6, 0, Math.max(0.05, 0.14 - (i % 6) * 0.012) * ramp);
    }
  } else {
    const tree = kind === "tree", inset = tree ? Math.max(2, 2.25 * s) : 0;
    const row = (tree ? 26 : 42) * s, barHeight = Math.max(4, 13 * s * 0.72);
    const stub = Math.max(4, Math.min(row, (tree ? 13 : 10) * s)), gap = (tree ? 8 : 7) * s;
    for (let i = 0; i < widths.length; i++) {
      const y = inset + i * row; if (y + row > height + (tree ? 0 : 0.5)) break;
      const x = inset + 12 * s + (tree ? (depths[i] + 1) * 14 * s : 0), barX = x + stub + gap;
      add(x, y + (row - stub) / 2, stub, stub, 3 * s, alpha(i));
      add(barX, y + (row - barHeight) / 2, Math.max(0, width - inset - 12 * s - barX) * widths[i], barHeight, barHeight / 2, alpha(i));
    }
  }
  return bars;
}

/** Native GPU paint only; never takes keyboard focus or pointer input. */
export function LoadingSkeleton(props: { config: AppConfig; kind: SkeletonKind; style?: StyleDesc; initialSize?:{width:number;height:number}; immediate?:boolean }) {
  const [size, setSize] = createSignal(props.initialSize||{ width: 0, height: 0 });
  const initialElapsed=props.immediate ? .18 : 0;
  const started = performance.now()-initialElapsed*1000, [elapsed, setElapsed] = createSignal(initialElapsed);
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const advance = () => {
    const now = performance.now(), seconds = (now - started) / 1000;
    setElapsed(seconds);
    if (!props.config.reducedMotion && (props.kind !== "document" || seconds < 0.32)) animation.request(advance, now);
  };
  onMount(() => { if (!props.config.reducedMotion) animation.request(advance, started); });
  onCleanup(() => animation.dispose());
  const bars = createMemo(() => {
    const { width, height } = size();
    return width > 0 && height > 0 ? loadingSkeletonBars(props.kind, width, height, props.config.scale, props.config.reducedMotion ? 1 : elapsed()) : [];
  });
  const color = (alpha: number) => {
    const hex = (props.kind === "document" ? props.config.theme.dim : props.config.theme.muted).replace(/^#/, "");
    const rgb = [0, 2, 4].map(index => parseInt(hex.slice(index, index + 2), 16));
    return `rgba(${rgb.join(",")},${alpha * (hex.length === 8 ? parseInt(hex.slice(6, 8), 16) / 255 : 1)})`;
  };
  return <div testId={`loading-skeleton:${props.kind}`} aria-label="Loading" aria-busy={true}
    onLayout={event => setSize(previous => previous.width === (event.width || 0) && previous.height === (event.height || 0) ? previous : { width: event.width || 0, height: event.height || 0 })}
    style={{ position: "absolute", left: 0, right: 0, top: 0, bottom: 0, overflow: "hidden", pointerEvents: "none", ...props.style }}>
    <Index each={bars()}>{bar => <div style={{ position: "absolute", left: bar().x, top: bar().y, width: bar().width, height: bar().height,
      borderRadius: bar().radius, backgroundColor: color(bar().alpha), pointerEvents: "none" }} />}</Index>
  </div>;
}
