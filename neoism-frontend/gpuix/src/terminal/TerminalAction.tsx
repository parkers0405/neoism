import { createEffect, createMemo, createSignal, onCleanup, Show, untrack } from "solid-js";
import type { AppConfig } from "../config";
import { cssColor, type CanvasOp } from "../ui/canvas";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { terminalIcons, type TerminalIcon } from "./icons";

export function TerminalAction(props: { config: AppConfig; label: string; icon: TerminalIcon; action: () => void; testId?: string; active?: boolean; disabled?: boolean; tooltip?: boolean; color?: string; onHover?: (over: boolean) => void }) {
  const c = props.config.theme, s = props.config.scale, animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const [over, setOver] = createSignal(false), [mix, setMix] = createSignal(0), [tip, setTip] = createSignal(false);
  const [bounds, setBounds] = createSignal({ x: 0, y: 0, width: 26 * s, height: 26 * s });
  let delay: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    const target = over() ? 1 : 0, start = performance.now(), from = untrack(mix);
    clearTimeout(delay); setTip(false); animation.cancel();
    if (over() && props.tooltip !== false) delay = setTimeout(() => setTip(true), 400);
    if (props.config.reducedMotion) { setMix(target); return; }
    const frame = () => { const t = Math.min(1, (performance.now() - start) / 100); setMix(from + (target - from) * (1 - (1 - t) ** 3)); if (t < 1) animation.request(frame); };
    animation.request(frame);
  });
  onCleanup(() => { animation.dispose(); clearTimeout(delay); });
  const ink = createMemo(() => {
    const ops: CanvasOp[] = [];
    if (!props.disabled && (mix() > 0 || props.active)) ops.push({ k: "r", b: [0, 0, 26 * s, 26 * s], r: [5 * s, 5 * s, 5 * s, 5 * s], c: cssColor(c.fg, .085 * Math.max(props.active ? .5 : 0, mix())) });
    return JSON.stringify(ops);
  });
  const tipWidth = () => Math.max(100 * s, props.label.length * 6.4 * s + 16 * s);
  const hover = (value: boolean) => { setOver(value); props.onHover?.(value); };
  return <div testId={props.testId} role="button" tabIndex={props.disabled ? -1 : 0} aria-label={props.label} aria-disabled={props.disabled} data-preserve-keyboard-focus={true}
    onMouseEnter={() => hover(true)} onMouseLeave={() => hover(false)} onFocus={() => hover(true)} onBlur={() => hover(false)}
    onClick={() => { if (!props.disabled) props.action(); }} onKeyDown={event => { if (event.key === "enter" || event.key === "space") { event.preventDefault(); event.stopPropagation(); if (!props.disabled) props.action(); } }}
    onLayout={event => setBounds({ x: event.x || 0, y: event.y || 0, width: event.width || 26 * s, height: event.height || 26 * s })}
    style={{ width: 26 * s, height: 26 * s, flexShrink: 0, position: "relative", cursor: props.disabled ? "not-allowed" : "pointer" }}>
    <canvas commands={ink()} style={{ position: "absolute", left: 0, top: 0, width: 26 * s, height: 26 * s, pointerEvents: "none" }}/>
    <svg source={terminalIcons[props.icon]} style={{ position: "absolute", left: 5 * s, top: 5 * s, width: 16 * s, height: 16 * s, color: props.color || (props.active ? c.accent : c.fg), opacity: props.disabled ? .25 : over() || props.active ? .95 : .6, pointerEvents: "none" }}/>
    <Show when={tip() && props.tooltip !== false}><anchored fill="window" deferred={true} priority={5} occlude={false} style={{ pointerEvents: "none", backgroundColor: "transparent" }}>
      <div style={{ position: "absolute", left: Math.max(8 * s, bounds().x + bounds().width - tipWidth()), top: bounds().y < 50 * s ? bounds().y + bounds().height + 5 * s : bounds().y - 29 * s,
        width: tipWidth(), height: 24 * s, backgroundColor: c.surface, borderWidth: 1, borderColor: c.border, borderRadius: 4 * s, paddingLeft: 8 * s, display: "flex", alignItems: "center" }}>
        <text style={{ color: c.fg, fontFamily: props.config.fontFamily, fontSize: 11 * s, whiteSpace: "nowrap" }}>{props.label}</text>
      </div>
    </anchored></Show>
  </div>;
}