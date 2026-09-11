import { useEffect, useState } from "react";
import { FX, type FxKind } from "../fx";

function Fella({ x, y = 205, color = "#64c6cd", flip = false, arms = false, bow = false }: { x: number; y?: number; color?: string; flip?: boolean; arms?: boolean; bow?: boolean }) {
    return <g transform={`translate(${x} ${y}) scale(${flip ? -1 : 1} 1) rotate(${bow ? 35 : 0})`}>
        <path fill="#433029" d="M-9-48h18v8H-9z" /><path fill="#e9b391" d="M-8-40h16v14H-8z" />
        <path fill={color} d="M-10-26h20v22h-20z" />
        <path stroke="#e9b391" strokeWidth="6" fill="none" d={arms ? "M-10-22l-10-15M10-22l12-15" : "M-10-22l-5 18M10-22l8 16"} />
        <path stroke="#4b527a" strokeWidth="8" d="M-5-4l-3 19M5-4l5 19" /><path stroke="#242334" strokeWidth="7" d="M-8 15h-7M10 15h7" />
    </g>;
}
export function FxScene({ kind }: { kind: FxKind }) {
    const [t, setT] = useState(0);
    useEffect(() => {
        const start = performance.now(); let frame = 0;
        function tick(now: number) { const seconds = (now - start) / 1000; setT(seconds); if (seconds < FX[kind].seconds) frame = requestAnimationFrame(tick); }
        frame = requestAnimationFrame(tick);
        return () => cancelAnimationFrame(frame);
    }, [kind]);
    const end = FX[kind].seconds;
    const x = t < 1.6 ? 650 - t / 1.6 * 280 : t > end - 1.4 ? 370 + (t - end + 1.4) * 220 : 370;
    const beat = Math.floor(t * 7) % 2 === 0;
    return <div className={`fx-scene fx-${kind}`} role="img" aria-label={FX[kind].label}>
        <svg viewBox="0 0 640 280" aria-hidden="true">
            {kind === "piss" && <><Fella x={x} flip /><g opacity={t >= 2.4 && t < 5.6 ? 1 : 0}><path d="M350 194Q300 158 248 239" fill="none" stroke="#efd448" strokeWidth="3" strokeDasharray={beat ? "4 6" : "6 4"} /><ellipse cx="243" cy="244" rx={Math.max(0, t-2.4)*13} ry="5" fill="#efd448" opacity=".5" /></g></>}
            {kind === "cuss" && <><Fella x={x} arms={t > 1.6 && t < 4.9 && beat} />{t > 1.6 && t < 4.9 && <g><path d="M220 92h220v53h-65l-12 17v-17H220z" fill="#f8e2cf" /><text x="330" y="128" textAnchor="middle" fill="#b12f41" fontSize="29">{beat ? "@#$%!!" : "&!#@?!"}</text></g>}</>}
            {kind === "glitch" && <><path d={t > 1.2 && t < 2.6 ? "M0 211h300l30-40m45 40h265" : "M0 211h640"} stroke="#6fd8ce" fill="none" strokeWidth="5" /><Fella x={x} arms={t > .9 && t < 3} />{t > 1.2 && t < 2.6 && Array.from({ length: 16 }, (_, i) => <rect key={i} x={(i*73 + t*300)%640} y={i*17} width={80+i*8} height={beat ? 4 : 8} fill={i%2 ? "#f36cac" : "#6cf3ec"} opacity=".6" />)}</>}
            {kind === "disco" && <><g transform={`translate(320 ${Math.min(45, -50+t*120)})`}><path d="M0-100v100" stroke="#ccc" /><circle r="26" fill="#aeb8db" /><path d="M-24-8h48M-24 8h48M-9-24v48M9-24v48" stroke="#fff" />{["#e077bf", "#82dcdb", "#e8cb6d"].map((c,i) => <path key={c} d={`M0 24L${Math.sin(t*2+i*2)*360} 240l100 0z`} fill={c} opacity=".18" />)}</g><Fella x={x + Math.sin(t*8)*12} arms={beat} />{Array.from({length: 35}, (_,i) => <rect key={i} x={(i*79)%640} y={(t*65+i*17)%270} width="4" height="7" fill={["#e077bf", "#82dcdb", "#e8cb6d"][i%3]} />)}</>}
            {kind === "gangfight" && <>{[0,1,2].map(i => <g key={i}><Fella x={Math.min(125, t*80)-i*40} color="#797fe6" arms={t>2.2} /><g opacity={t < 6 ? 1 : .2} transform={t >= 6 ? `translate(0 ${35+i*5})` : undefined}><Fella x={640-Math.min(125,t*80)+i*40} color="#cf7068" flip arms={t>2.2} /></g></g>)}{t>2.2 && t<6 && <g stroke="#ffda72" strokeWidth="3">{[0,1,2,3].map(i => <path key={i} d={`M${(t*500+i*140)%520+60} ${180+i*6}h25`} />)}</g>}{t>=6 && <Fella x={220+(t-6)*140} color="#797fe6" />}</>}
            {kind === "praise" && <><path d="M280 10L120 260h400L360 10z" fill="#edce74" opacity=".15" /><path d="M280 105h80v123h-80zM263 155h17v85h-17zM360 155h17v85h-17z" fill="#b18b38" /><ellipse cx="320" cy="72" rx="23" ry="7" fill="none" stroke="#ffdf77" strokeWidth="4" /><Fella x={320} y={140} color="#fff2d7" arms /><path d="M310 136l-15 59h50l-15-59" fill="#fff2d7" />{[0,1,2,3,4,5].map(i => <Fella key={i} x={130+i*76} y={251} bow={Math.sin(t*3+i)>.1} color={i%2 ? "#ac8bc9" : "#799ba3"} />)}{[0,1,2,3].map(i => <text key={i} x={230+i*65} y={210-(t*25+i*24)%180} fill="#f1d27b" fontSize="22">♪</text>)}</>}
        </svg>
    </div>;
}
