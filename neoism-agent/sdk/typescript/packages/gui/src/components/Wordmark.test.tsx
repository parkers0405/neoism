import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { Avatar, Logo, Wordmark } from './Identity';
import { wordmarkFrame } from './wordmarkMotion';
import { wordmarkGeometry as g } from '../generated/wordmark';
const zero = [0, 0, 0, 0, 0, 0];
describe('native full wordmark', () => {
    it('regenerates exactly from both SVG sources and Rust constants', () => {
        execFileSync(process.execPath, ['scripts/generate-wordmark.mjs', '--check']);
    });
    it('renders all six independent three-layer SVG letters, not font text', () => {
        const html = renderToStaticMarkup(<Wordmark />);
        expect(html).toContain('aria-label="NEOISM"');
        expect(html.match(/data-wordmark-letter=/g)).toHaveLength(6);
        expect(html.match(/<path /g)).toHaveLength(18);
        expect(html).not.toContain('<text');
        expect(html).toContain('aria-hidden="true"');
        expect(renderToStaticMarkup(<Logo />)).toContain('hero-logo');
        expect(renderToStaticMarkup(<Avatar seed="native" />)).toContain('crispEdges');
    });
    it('targets every letter independently and exponentially releases hover', () => {
        for (let i = 0; i < 6; i++) {
            const frame = wordmarkFrame(zero, [(i + .5) * g.width / 6, g.height / 2], .1, 0);
            expect(frame.letters[i].hover).toBeCloseTo(1 - Math.exp(-1.4));
            expect(frame.letters.filter(l => l.hover > 0)).toHaveLength(1);
            expect(frame.letters[i].cy).toBeLessThan(g.height / 2);
            const released = wordmarkFrame(frame.letters.map(l => l.hover), null, .1, 0);
            expect(released.letters[i].hover).toBeCloseTo(frame.letters[i].hover * Math.exp(-1.4));
        }
    });
    it('matches phase-offset sine shimmer, bounded dt, squash and ripple', () => {
        const frame = wordmarkFrame(zero, null, 1, 0);
        expect(frame.letters[0].scale).toBe(1);
        expect(frame.letters[1].scale).toBeCloseTo(1 + Math.sin(.16 * Math.PI * 2) * .025);
        const pressed = wordmarkFrame(zero, null, 0, 0, 460 * .22);
        expect(pressed.letters[0].scale).toBeCloseTo(.92);
        expect(wordmarkFrame(zero, null, 0, 0, 0).opacity).toBe(.22);
        expect(wordmarkFrame(zero, null, 0, 0, 280).opacity).toBe(0);
        expect(wordmarkFrame(zero, null, 0, 0, 461).letters[0].scale).toBe(1);
    });
    it('disables hover, shimmer, squash and ripple for reduced motion', () => {
        const frame = wordmarkFrame([1,1,1,1,1,1], [10,10], .1, 2, 100, true);
        expect(frame.opacity).toBe(0);
        for (const l of frame.letters) { expect(l.scale).toBe(1); expect(l.hover).toBe(0); }
        expect(readFileSync('src/components/Wordmark.css', 'utf8')).toContain('prefers-reduced-motion: reduce');
    });
});
