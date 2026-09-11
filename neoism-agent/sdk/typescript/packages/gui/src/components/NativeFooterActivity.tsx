import { memo, useEffect, useRef } from "react";
import "./nativeFooterActivity.css";

/** shared/render_policy/blocks.rs::opencode_scanner_frame: 54 frames, 40ms each. */
export function footerScannerFrame(seconds: number) {
    const frame = Number.isFinite(seconds) ? ((Math.floor(seconds * 25 + .0001) % 54) + 54) % 54 : 0;
    const forward = frame < 17;
    const holding = (frame >= 8 && frame < 17) || frame >= 24;
    const hold = frame < 17 ? frame - 8 : frame - 24;
    const head = frame < 8 ? frame : frame < 17 ? 7 : frame < 24 ? 23 - frame : 0;
    const progress = frame < 8 ? frame / 7 : (frame - 17) / 6;
    const fade = holding ? Math.max(.3, 1 - hold / (forward ? 9 : 30) * .7) : .3 + progress * .7;
    return Array.from({ length: 8 }, (_, index) => {
        const distance = forward ? head - index : index - head;
        const trail = holding ? distance + hold : distance >= 0 && distance < 6 ? distance : -1;
        const active = trail >= 0 && trail < 6;
        const brightness = active && trail === 1 ? 1.15 : 1;
        return { active, alpha: Math.round((active ? trail === 0 ? 1 : trail === 1 ? .9 : .65 ** (trail - 1) : .6 * fade) * 255) / 255,
            bloom: active ? .10 + Math.max(0, brightness - 1) * .60 : 0 };
    });
}

/** The help-strip scanner, NOT the three dots following the activity word.
 * user_input.rs draws 12.5px em cells: 8.5px pitch, 6.5/2.75px square faces,
 * bg +3px, dim +1.5px, then the magenta thinking face bloomed toward fg.
 * CSS native theme roles stay live when PixelDots/appearance changes.
 */
export const NativeFooterActivity = memo(function NativeFooterActivity({ busy }: { busy: boolean }) {
    const strip = useRef<HTMLSpanElement>(null);
    useEffect(() => {
        if (!busy || !strip.current) return;
        const cells = Array.from(strip.current.children) as HTMLElement[];
        const motion = window.matchMedia?.("(prefers-reduced-motion: reduce)");
        let timer: ReturnType<typeof setInterval> | undefined;
        const start = performance.now();
        const paint = () => footerScannerFrame(motion?.matches ? 0 : (performance.now() - start) / 1000).forEach((cell, i) => {
            cells[i].style.setProperty("--scanner-size", `${cell.active ? 6.5 : 2.75}px`);
            cells[i].style.setProperty("--scanner-alpha", `${cell.alpha}`);
            cells[i].style.setProperty("--scanner-bloom", `${cell.bloom * 100}%`);
        });
        const schedule = () => { clearInterval(timer); paint(); if (!motion?.matches) timer = setInterval(paint, 40); };
        schedule(); motion?.addEventListener("change", schedule);
        return () => { clearInterval(timer); motion?.removeEventListener("change", schedule); };
    }, [busy]);
    if (!busy) return null;
    return <span className="native-footer-activity" role="status" aria-label="Agent running">
        <span ref={strip} className="native-footer-scanner" aria-hidden="true">
            {Array.from({ length: 8 }, (_, i) => <span key={i} className="native-footer-cell"><i /><b /><em /></span>)}
        </span>
    </span>;
});
