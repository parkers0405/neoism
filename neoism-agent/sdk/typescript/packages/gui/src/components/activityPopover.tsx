import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { Square, X } from "lucide-react";
import type { SessionActivity } from "../useSessionActivity";
import { elapsedLabel } from "./nativeActivityMotion";

export function ActivityPopover({ anchor, kind, activity, close, style }: {
    anchor: HTMLButtonElement; kind: "queue" | "background"; activity: SessionActivity; close(): void; style?: CSSProperties;
}) {
    const panel = useRef<HTMLDivElement>(null);
    const [position, setPosition] = useState({ left: 0, bottom: 0, maxHeight: 440 });
    const [, tick] = useState(0);
    useLayoutEffect(() => {
        const place = () => { const r = anchor.getBoundingClientRect(); setPosition({ left: Math.max(8, Math.min(r.left, window.innerWidth - 368)), bottom: Math.max(8, window.innerHeight - r.top + 6), maxHeight: Math.max(0, Math.min(440, r.top - 14)) }); };
        place(); window.addEventListener("resize", place);
        panel.current?.focus();
        return () => { window.removeEventListener("resize", place); };
    }, [anchor]);
    useEffect(() => {
        const outside = (e: PointerEvent) => { if (!panel.current?.contains(e.target as Node) && !anchor.contains(e.target as Node)) close(); };
        const escape = (e: KeyboardEvent) => { if (e.key === "Escape") { e.preventDefault(); close(); anchor.focus(); } };
        document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape);
        const timer = setInterval(() => tick(n => n + 1), 1000);
        return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); clearInterval(timer); };
    }, [anchor, close]);
    return createPortal(<div ref={panel} tabIndex={-1} role="dialog" aria-label={kind === "queue" ? "Queued messages" : "Background tasks"} className="activity-popover" style={{ ...style, ...position }}>
        <header><strong>{kind === "queue" ? "Queued messages" : "Background tasks"}</strong><button aria-label="Close activity" onClick={() => { close(); anchor.focus(); }}><X size={14} /></button></header>
        {activity.error && <div role="alert">{activity.error}<button onClick={() => void activity.refresh()}>Retry</button></div>}
        {kind === "queue" ? <>
            {activity.loading ? <div className="activity-skeleton" aria-label="Loading queued messages" /> : <>
                <small>{activity.queue?.running ? "Running" : "Idle"}{activity.queue?.worker ? " · Queue worker active" : ""}</small>
                <ul>{activity.queue?.items.map(item => <li key={item.index}><details><summary>#{item.index + 1} · {item.text?.slice(0, 100) || `${item.partCount} non-text parts`}</summary><p>{item.text || "No text preview available"}</p>{item.agent && <small>{item.agent}</small>}</details></li>)}</ul>
                {!activity.queue?.count && <p>No queued messages</p>}
                <footer><button disabled={!activity.queue?.count || !!activity.pending} onClick={() => void activity.mutate("pop")}>{activity.pending === "pop" ? "Removing…" : "Remove next"}</button><button disabled={!activity.queue?.count || !!activity.pending} onClick={() => void activity.mutate("clear")}>{activity.pending === "clear" ? "Clearing…" : "Clear queue"}</button></footer>
            </>}
        </> : <ul>{activity.jobs.length ? activity.jobs.map(job => {
            const key = `${job.sessionId}/${job.jobId}`, stopping = activity.pending === key || activity.stopping.includes(key);
            return <li key={key}><div className="activity-job"><div><strong>Background shell task</strong><small>{stopping ? "Stopping" : "Running"} · {elapsedLabel((Date.now() - job.startedAt) / 1000)}</small><details><summary>{job.jobId}</summary><p>Session: {job.sessionId}</p><p>Command preview is not supplied by this runtime API.</p></details></div><button aria-label={`Stop job ${job.jobId}`} disabled={!!activity.pending || stopping} onClick={() => void activity.mutate("stop", job)}><Square size={12} />{stopping ? "Stopping…" : "STOP"}</button></div></li>;
        }) : <li>No running background tasks</li>}</ul>}
    </div>, document.body);
}
