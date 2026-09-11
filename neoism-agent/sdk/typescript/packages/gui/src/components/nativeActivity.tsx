import { memo, useEffect, useRef, useState, type CSSProperties } from "react";
import type { SessionActivity } from "../useSessionActivity";
import { ActivityPopover } from "./activityPopover";
import type { MessageWithParts, SessionRuntimeSnapshot } from "@neoism/sdk";
import { ACTIVITY, STATUS, activityLabel, dotFrame, elapsedLabel, glyphFrame, phaseSeconds, resolveActivity, runtimeElapsed, type NativeActivityState } from "./nativeActivityMotion";
import "./nativeActivity.css";

export interface NativeActivityProps {
    sessionActivity?: SessionActivity;
    busy: boolean;
    messages?: MessageWithParts[];
    runtime?: SessionRuntimeSnapshot;
    /** Optional authoritative native status (runtime has no reasoning/retry/compaction enum). */
    activity?: NativeActivityState;
    /** Raw native theme roles, before GUI muted/dim semantic remapping. */
    palette?: NativeActivityState["palette"];
    sessionId?: string;
}
const EMPTY: MessageWithParts[] = [];
const WORD_FONT = 'bold 12px "Press Start 2P", monospace';
const UI_FONT = '"Neoism Geist Mono", "Geist Mono", monospace';

/** Canvas owns every animation frame; neither React nor the transcript updates at 60 Hz. */
export const NativeActivity = memo(function NativeActivity({ busy, messages = EMPTY, runtime, activity, palette: themePalette, sessionId, sessionActivity }: NativeActivityProps) {
    const [popover, setPopover] = useState<{ kind: "queue" | "background"; anchor: HTMLButtonElement; scope: SessionActivity["scope"] }>();
    useEffect(() => { setPopover(undefined); }, [sessionActivity?.scope, sessionId]);
    const resolved = resolveActivity(messages, busy, runtime, activity, sessionId);
    const queued = sessionActivity ? sessionActivity.queue?.count ?? 0 : resolved.queuedCount ?? 0;
    const background = sessionActivity ? sessionActivity.jobs.length : resolved.backgroundCount ?? 0;
    const state = resolved.status === "idle" && background > 0 ? { ...resolved, status: "backgroundTasks" as const } : resolved;
    const label = activityLabel(state), canvas = useRef<HTMLCanvasElement>(null);
    const live = useRef({ state, runtime }); live.current = { state, runtime };
    const clock = useRef({ sessionId, started: 0, sample: undefined as SessionRuntimeSnapshot | undefined, sampleAt: 0, elapsed: 0 });
    if (clock.current.sessionId !== sessionId) clock.current = { sessionId, started: 0, sample: undefined, sampleAt: 0, elapsed: 0 };
    useEffect(() => {
        const el = canvas.current;
        if (!el || !label) { clock.current.started = 0; return; }
        const ctx = el.getContext("2d");
        if (!ctx) return;
        let disposed = false, frame = 0, timer: ReturnType<typeof setInterval> | undefined;
        const start = performance.now();
        if (!clock.current.started) clock.current.started = start;
        const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
        let width = el.parentElement?.clientWidth || 320;
        // Resolve CSS theme roles only when appearance changes, never per glyph/frame.
        let palette: Record<string, number[]> = {};
        let uiFont = UI_FONT;
        const swatch = document.createElement("canvas"); swatch.width = swatch.height = 1;
        const colorContext = swatch.getContext("2d");
        function colors() {
            const css = getComputedStyle(el!);
            uiFont = css.getPropertyValue("--font-code").trim() || UI_FONT;
            const defaults: Record<string, string> = { bg: "#000000", fg: "#e8e8e8", dim: "#b0b0b0", muted: "#5a5a5a", accent: "#e8e8e8", magenta: "#c2a2e3", yellow: "#fbdf90", green: "#9fe8c3", red: "#ef8891" };
            palette = Object.fromEntries(Object.entries(defaults).map(([role, fallback]) => {
                const value = css.getPropertyValue(`--theme-${role}`).trim() || css.getPropertyValue(`--${role === "dim" ? "text-faint" : role}`).trim() || fallback;
                if (!colorContext) return [role, [232, 232, 232]];
                colorContext.clearRect(0, 0, 1, 1); colorContext.fillStyle = fallback; colorContext.fillStyle = value;
                colorContext.fillRect(0, 0, 1, 1);
                return [role, [...colorContext.getImageData(0, 0, 1, 1).data].slice(0, 3)];
            }));
        }
        const rgb = (v: number[], alpha = 1) => `rgba(${v.join(",")},${alpha})`;
        function draw(now: number) {
            if (disposed || !ctx || !el) return;
            const phase = phaseSeconds(now), transition = (now - start) / 1000;
            ctx.font = WORD_FONT;
            const available = Math.max(ACTIVITY.minWidth, width - ACTIVITY.reserve);
            const lines: string[] = []; let line = "";
            // Native wrap_input_text: wrap at words, splitting an overlong word at glyph boundaries.
            for (const word of label.split(" ")) {
                if (line && ctx.measureText(`${line} ${word}`).width > available) { lines.push(line); line = ""; }
                for (const ch of (line ? " " : "") + word) {
                    if (line && ctx.measureText(line + ch).width > available) { lines.push(line); line = ""; }
                    line += ch;
                }
            }
            if (line) lines.push(line);
            const height = Math.max(1, lines.length) * ACTIVITY.lineHeight;
            const dpr = window.devicePixelRatio || 1;
            if (el.width !== Math.round(width * dpr) || el.height !== Math.round(height * dpr)) {
                el.width = Math.round(width * dpr); el.height = Math.round(height * dpr); el.style.height = `${height}px`;
            }
            ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, width, height);
            ctx.font = WORD_FONT; ctx.textBaseline = "alphabetic";
            const metrics = ctx.measureText("M");
            const baseline = 7 + (metrics.fontBoundingBoxAscent || metrics.actualBoundingBoxAscent || 12);
            const accent = palette[STATUS[live.current.state.status][2]], fg = palette.fg;
            let ix = 0, x = 0, y = baseline, trailing = accent;
            for (let row = 0; row < lines.length; row++) {
                x = ACTIVITY.inset; y = baseline + row * ACTIVITY.lineHeight;
                for (const target of lines[row]) {
                    const g = glyphFrame(target, ix++, [...label].length, phase, transition, reduced.matches);
                    trailing = accent.map((v, i) => Math.round(v + (fg[i] - v) * g.mix));
                    ctx.fillStyle = rgb(palette.bg, 210 / 255); ctx.fillText(g.char, x + g.x + 3.5, y + g.y + 3.5);
                    ctx.fillStyle = rgb(palette.dim, 240 / 255); ctx.fillText(g.char, x + g.x + 1.75, y + g.y + 1.75);
                    ctx.fillStyle = rgb(trailing); ctx.fillText(g.char, x + g.x, y + g.y);
                    x += ctx.measureText(g.char).width;
                }
            }
            x += (reduced.matches ? 0 : Math.sin(phase * 3) * 1.8) + 7;
            ctx.font = `bold 14px ${uiFont}`;
            for (let i = 0; i < 3; i++) {
                const dot = dotFrame(i, phase, reduced.matches);
                // Rust u8::saturating_mul(5)/6 is intentional (not alpha * 5/6).
                ctx.fillStyle = rgb(palette.bg, Math.floor(Math.min(255, Math.round(dot.alpha * 255) * 5) / 6) / 255);
                ctx.fillText(".", x + dot.x + 3, y + dot.y + 3);
                ctx.fillStyle = rgb(palette.dim, dot.alpha); ctx.fillText(".", x + dot.x + 1.5, y + dot.y + 1.5);
                ctx.fillStyle = rgb(trailing, dot.alpha); ctx.fillText(".", x + dot.x, y + dot.y);
                x += ctx.measureText(".").width + 2;
            }
            const c = clock.current, current = live.current;
            if (c.sample !== current.runtime) {
                c.sample = current.runtime; c.sampleAt = now;
                c.elapsed = runtimeElapsed(current.runtime, Date.now()) ?? (now - c.started) / 1000;
            }
            const e = current.runtime?.execution;
            const rate = e ? Object.keys(e.activeSegments).length : 1;
            const elapsed = current.state.elapsedSeconds ?? (e ? c.elapsed + (now - c.sampleAt) / 1000 * rate : (now - c.started) / 1000);
            ctx.font = `italic 13px ${uiFont}`; ctx.fillStyle = rgb(palette.muted);
            ctx.fillText(`· ${elapsedLabel(elapsed)} model · ${STATUS[current.state.status][1]}`, x + 8, y);
        }
        function tick(now: number) { if (disposed) return; draw(now); frame = requestAnimationFrame(tick); }
        function schedule() {
            cancelAnimationFrame(frame); if (timer) clearInterval(timer);
            draw(performance.now());
            if (reduced.matches) timer = setInterval(() => draw(performance.now()), 1000);
            else frame = requestAnimationFrame(tick);
        }
        colors(); schedule();
        reduced.addEventListener("change", schedule);
        const resize = typeof ResizeObserver !== "undefined" ? new ResizeObserver(entries => { width = entries[0].contentRect.width; draw(performance.now()); }) : undefined;
        if (el.parentElement) resize?.observe(el.parentElement);
        const theme = new MutationObserver(() => { colors(); draw(performance.now()); });
        // Observe ancestors only: our own canvas height writes must not trigger this observer.
        for (let ancestor = el.parentElement; ancestor; ancestor = ancestor.parentElement) theme.observe(ancestor, { attributes: true, attributeFilter: ["class", "style"] });
        void document.fonts?.ready.then(() => { if (!disposed) draw(performance.now()); });
        return () => { disposed = true; cancelAnimationFrame(frame); if (timer) clearInterval(timer); resize?.disconnect(); theme.disconnect(); reduced.removeEventListener("change", schedule); };
    }, [label, sessionId]);
    if (!label && !queued && !background && !popover) return null;
    const open = (kind: "queue" | "background", anchor: HTMLButtonElement) => {
        if (sessionActivity) { setPopover(popover?.kind === kind ? undefined : { kind, anchor, scope: sessionActivity.scope }); void sessionActivity.refresh(); }
    };
    const paletteStyle = Object.fromEntries(Object.entries(themePalette ?? state.palette ?? {}).map(([role, value]) => [`--theme-${role}`, value])) as CSSProperties;
    return <div className="native-activity" style={paletteStyle} role="status" aria-live="polite" aria-atomic="true">
        <span className="native-activity-accessible">{label}</span>
        {label && <canvas ref={canvas} aria-hidden="true" />}
        {queued > 0 && <button type="button" disabled={!sessionActivity} aria-haspopup="dialog" aria-expanded={popover?.kind === "queue"} onClick={e => open("queue", e.currentTarget)} className="native-activity-queue">{background ? "├─" : "╰─"} {queued === 1 ? "queued message" : `queued messages (${queued})`}</button>}
        {background > 0 && <button type="button" disabled={!sessionActivity} aria-haspopup="dialog" aria-expanded={popover?.kind === "background"} onClick={e => open("background", e.currentTarget)} className="native-activity-background">╰─ {background} background {background === 1 ? "task" : "tasks"} running</button>}
        {popover && sessionActivity && popover.scope === sessionActivity.scope && <ActivityPopover anchor={popover.anchor} kind={popover.kind} activity={sessionActivity} close={() => setPopover(undefined)} style={paletteStyle} />}
    </div>;
});
