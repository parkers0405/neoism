import { useEffect, useId, useRef } from "react";
import { wordmarkLetters, wordmarkGeometry as geometry } from "../generated/wordmark";
import { wordmarkFrame } from "./wordmarkMotion";
import "./Wordmark.css";
import { avatarCells, avatarGridSize } from "../generated/avatar";
import { neoismLogoPath, neoismLogoViewBox } from "../generated/logo";
export function Avatar({ seed }: { seed: string }) {
    const cells = avatarCells(seed);
    const width = avatarGridSize(seed);
    const height = width;
    return (
        <svg
            className="avatar"
            viewBox={`0 0 ${width} ${height}`}
            role="img"
            aria-label={`${seed}'s avatar`}
            shapeRendering="crispEdges"
        >
            {cells.map((c, i) => (
                <rect
                    key={i}
                    x={c.x}
                    y={c.y}
                    width="1"
                    height="1"
                    fill={c.color}
                />
            ))}
        </svg>
    );
}
export function Logo() {
    return (
        <svg
            className="hero-logo"
            viewBox={neoismLogoViewBox}
            role="img"
            aria-label="Neoism"
        >
            <path d={neoismLogoPath} fill="currentColor" />
        </svg>
    );
}

/** Full native six-letter identity. Parent owns placement above the home composer. */
export function Wordmark({ className = "" }: { className?: string }) {
    const id = useId();
    const svg = useRef<SVGSVGElement>(null);
    const mouse = useRef<[number, number] | null>(null);
    const click = useRef<{ at: number; x: number; y: number } | null>(null);
    const reduced = useRef(false);
    useEffect(() => {
        const node = svg.current!;
        const media = window.matchMedia('(prefers-reduced-motion: reduce)');
        const letters = Array.from(node.querySelectorAll<SVGGElement>('[data-wordmark-letter]'));
        const glows = Array.from(node.querySelectorAll<SVGGElement>('[data-wordmark-glow]'));
        const ripple = node.querySelector('circle')!;
        let hover = Array(6).fill(0), last = 0, frame = 0;
        const paint = (now: number) => {
            const elapsed = click.current ? now - click.current.at : Infinity;
            const state = wordmarkFrame(hover, mouse.current, last ? (now - last) / 1000 : 0,
                (Date.now() / 1000) % 10000, elapsed, reduced.current);
            last = now;
            hover = state.letters.map(l => l.hover);
            state.letters.forEach((l, i) => {
                const center = (i + .5) * geometry.width / 6;
                letters[i].setAttribute('transform', `translate(${l.cx} ${l.cy}) scale(${l.scale}) translate(${-center} ${-geometry.height / 2})`);
                glows[i].setAttribute('transform', `translate(${center} ${geometry.height / 2}) scale(${l.glow}) translate(${-center} ${-geometry.height / 2})`);
            });
            ripple.setAttribute('r', String(state.radius));
            ripple.setAttribute('opacity', String(state.opacity));
            if (click.current) {
                ripple.setAttribute('cx', String(click.current.x));
                ripple.setAttribute('cy', String(click.current.y));
                if (elapsed > 460) click.current = null;
            }
            if (!reduced.current) frame = requestAnimationFrame(paint);
        };
        const preference = () => {
            cancelAnimationFrame(frame);
            reduced.current = media.matches;
            click.current = null;
            last = 0;
            paint(performance.now());
        };
        preference();
        media.addEventListener('change', preference);
        return () => { cancelAnimationFrame(frame); media.removeEventListener('change', preference); };
    }, []);
    const point = (clientX: number, clientY: number): [number, number] => {
        const matrix = svg.current?.getScreenCTM();
        if (!matrix) return [geometry.width / 2, geometry.height / 2];
        const p = new DOMPoint(clientX, clientY).matrixTransform(matrix.inverse());
        return [p.x, p.y];
    };
    return (
        <svg ref={svg} className={`neoism-home-wordmark ${className}`}
            viewBox={`0 0 ${geometry.width} ${geometry.height}`} role="img" aria-label="NEOISM"
            onPointerMove={e => { mouse.current = e.pointerType === 'touch' ? null : point(e.clientX, e.clientY); }}
            onPointerLeave={() => { mouse.current = null; }}
            onPointerCancel={() => { mouse.current = null; }}
            onClick={e => {
                if (reduced.current) return;
                const [x, y] = point(e.clientX, e.clientY);
                if (x >= 0 && x <= geometry.width && y >= 0 && y <= geometry.height)
                    click.current = { at: performance.now(), x, y };
            }}>
            <defs>
                {wordmarkLetters.map((layers, i) => (
                    <g key={i} id={`${id}-letter-${i}`}>
                        {layers.map((layer, j) => <path key={j} d={layer.d} fillOpacity={layer.opacity}
                            transform={`translate(0 ${-geometry.top}) ${layer.transform}`} />)}
                    </g>
                ))}
            </defs>
            <g aria-hidden="true" fill="currentColor" pointerEvents="none">
                <circle fill="white" opacity="0" />
                {wordmarkLetters.map((_, i) => (
                    <g key={i} data-wordmark-letter={i}>
                        <g data-wordmark-glow={i}><use href={`#${id}-letter-${i}`} /></g>
                        <use href={`#${id}-letter-${i}`} />
                    </g>
                ))}
            </g>
            {/* Stable native-style rectangular hit area; transformed ink never moves the target. */}
            <rect width={geometry.width} height={geometry.height} fill="transparent" aria-hidden="true" />
        </svg>
    );
}
