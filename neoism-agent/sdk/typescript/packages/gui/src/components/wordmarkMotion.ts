import { wordmarkMotion as m, wordmarkGeometry as g } from '../generated/wordmark';

/** Direct translation of shared agent_pane/view/wordmark.rs (seconds, SVG units). */
export function wordmarkFrame(previous: readonly number[], mouse: readonly [number, number] | null, dt: number, seconds: number, clickMs = Infinity, reduced = false) {
    const t = Math.min(1, Math.max(0, clickMs / 460));
    const press = t < .22 ? t / .22 : 1 - (t - .22) / .78;
    const squash = reduced ? 1 : 1 - .08 * Math.max(0, press);
    const w = g.width * squash, h = g.height * squash;
    const x = (g.width - w) / 2, y = (g.height - h) / 2;
    const alpha = 1 - Math.exp(-m.LETTER_HOVER_RATE * Math.max(0, Math.min(dt, .1)));
    const letters = previous.map((value, i) => {
        const lx = x + i * w / m.LETTER_COUNT;
        const target = mouse && mouse[0] >= lx && mouse[0] <= lx + w / m.LETTER_COUNT && mouse[1] >= y && mouse[1] <= y + h ? 1 : 0;
        const hover = reduced ? 0 : value + (target - value) * alpha;
        const shimmer = reduced ? 0 : Math.sin((seconds / m.LETTER_SHIMMER_PERIOD + i * .16) * Math.PI * 2) * m.LETTER_SHIMMER_AMP;
        const scale = squash * (1 + hover * m.LETTER_HOVER_SCALE + shimmer);
        return { hover, scale, glow: 1.04 + hover * .05,
            cx: x + (i + .5) * w / m.LETTER_COUNT,
            cy: y + h / 2 - hover * m.LETTER_HOVER_LIFT * h };
    });
    const rippleT = Math.min(1, Math.max(0, clickMs / 280));
    return { letters, radius: h * (.10 + .28 * rippleT), opacity: reduced ? 0 : .22 * (1 - rippleT) };
}
