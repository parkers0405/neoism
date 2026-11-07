import { createMemo, createSignal } from "solid-js";
import { motion } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { cssColor, type CanvasOp } from "./canvas";
import { easeOutCubicBezier } from "../design/animation";

/** Compact glass lens hosted in the workspace strip, never over document content. */
export function AgentDetailsButton(props: { config: AppConfig; height: number; open: boolean; onToggle: () => void }) {
    const s = Math.min(props.config.scale, Math.max(0, props.height - 4 * props.config.scale) / 44), c = props.config.theme;
    const size = 44 * s;
    const [hovered, setHovered] = createSignal(false), [pressed, setPressed] = createSignal(false);
    const ink = createMemo(() => {
      const ops: CanvasOp[] = [];
      const rect = (b: number[], color: number, radius: number, border?: number, stroke = 0) => ops.push({ k: "r", b: b.map(n => n * s), c: color, r: [radius, radius, radius, radius].map(n => n * s), ...(border === undefined ? {} : { bc: border, bw: stroke * s }) });
      // A translucent lens with an inset rim and top/bottom specular reflections.
      rect([2, 4, 40, 40], cssColor("#000000", .18), 20);
      rect([2, 2, 40, 40], cssColor(c.bg, hovered() ? .9 : .78), 20, cssColor(c.fg, hovered() ? .32 : .2), .8);
      rect([3.5, 3.5, 37, 37], cssColor(c.fg, hovered() ? .09 : .045), 18.5, cssColor(c.fg, .055), .6);
      rect([12, 4, 20, 1], cssColor(c.fg, .24), .5);
      rect([14, 39, 16, .8], cssColor(c.fg, .09), .4);
      const icon = cssColor(c.fg, hovered() ? 1 : .88);
      rect([12, 14, 20, 16], 0, 3, icon, 1.5);
      rect([26, 14.5, 1.3, 15], icon, .3);
      if (props.open) {
        rect([28, 16, 2.2, 12], cssColor(c.fg, .35), .8);
        rect([32, 6, 5, 5], cssColor(c.accent), 2.5);
      }
      return JSON.stringify(ops);
    });
    const label = () => `${props.open ? "Hide" : "Show"} agent details panel`;
    return <div testId="toggle-details" role="button" tabIndex={0} aria-label={label()} aria-expanded={props.open}
      data-preserve-keyboard-focus={true}
      onMouseMove={() => setHovered(true)} onMouseLeave={() => { setHovered(false); setPressed(false); }}
      onMouseDown={event => { if (event.button === undefined || event.button === 0) setPressed(true); }}
      onMouseUp={() => setPressed(false)} onClick={event => { if (event.button === undefined || event.button === 0) props.onToggle(); }}
      onFocus={() => setHovered(true)} onBlur={() => { setHovered(false); setPressed(false); }}
      onKeyDown={event => { if (["enter", "space", " "].includes(event.key?.toLowerCase() || "")) { event.preventDefault(); event.stopPropagation(); props.onToggle(); } }}
      style={{ position: "relative", width: 36 * props.config.scale, height: props.height, flexShrink: 0, cursor: "pointer", backgroundColor: "transparent" }}>
      <motion.div initial={false} animate={{ top: (props.height - size) / 2 + (pressed() ? 1 : hovered() ? -1 : 0) * s }} transition={{ duration: props.config.reducedMotion ? 0 : .18, ease: easeOutCubicBezier }}
        style={{ position: "absolute", left: (36 * props.config.scale - size) / 2, width: size, height: size, pointerEvents: "none" }}>
        <canvas commands={ink()} style={{ position: "absolute", left: 0, top: 0, width: size, height: size, pointerEvents: "none" }} />
      </motion.div>
    </div>;
}