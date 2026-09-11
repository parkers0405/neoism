import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import "./skeleton.css";

/** One accessible loading announcement per region; all placeholder geometry is decorative. */
export function Skeleton({ children, label = "Loading…", className = "" }: { children: ReactNode; label?: string; className?: string }) {
    const ref = useRef<HTMLDivElement>(null);
    const [visible, setVisible] = useState(true);
    useEffect(() => {
        const node = ref.current;
        if (!node) return;
        let intersecting = true;
        const update = () => setVisible(intersecting && !document.hidden);
        const observer = typeof IntersectionObserver === "undefined" ? undefined : new IntersectionObserver(([entry]) => {
            intersecting = entry.isIntersecting;
            update();
        });
        observer?.observe(node);
        document.addEventListener("visibilitychange", update);
        update();
        return () => { observer?.disconnect(); document.removeEventListener("visibilitychange", update); };
    }, []);
    return <div ref={ref} className={`skeleton ${className}`} aria-busy="true" data-paused={!visible || undefined} role="status">
        <span className="skeleton-label">{label}</span>
        <div aria-hidden="true" className="skeleton-shapes">{children}</div>
    </div>;
}
function Block({ width = "100%", className = "" }: { width?: string; className?: string }) {
    return <span className={`skeleton-block ${className}`} style={{ width } as CSSProperties} />;
}
const widths = ["72%", "53%", "84%", "63%", "77%", "46%"];
export function SkeletonRows({ kind, count = 4, header = false, label }: {
    kind: "session" | "provider" | "folder" | "setting";
    count?: number; header?: boolean; label?: string;
}) {
    return <Skeleton className={`skeleton-rows skeleton-${kind}`} label={label || `Loading ${kind === "session" ? "chats" : `${kind}s`}…`}>
        {header && <div className="skeleton-heading"><Block width="32%" /></div>}
        {Array.from({ length: count }, (_, i) => <div className="skeleton-row" key={i}>
            {kind !== "session" && <Block className="skeleton-icon" />}
            <div className="skeleton-copy"><Block width={widths[i % widths.length]} />{kind !== "session" && <Block width={widths[(i + 2) % widths.length]} className="skeleton-secondary" />}</div>
            {(kind === "provider" || kind === "setting") && <Block className="skeleton-control" />}
        </div>)}
    </Skeleton>;
}
export function ConversationSkeleton({ code = true }: { code?: boolean }) {
    return <Skeleton className="skeleton-conversation" label="Loading conversation…">
        <div className="skeleton-user"><Block width="86%" /><Block width="59%" /></div>
        <div className="skeleton-assistant"><Block width="26%" className="skeleton-secondary" /><Block width="94%" /><Block width="87%" /><Block width="65%" /></div>
        {code && <div className="skeleton-code"><Block width="31%" className="skeleton-secondary" /><Block width="72%" /><Block width="53%" /><Block width="81%" /></div>}
    </Skeleton>;
}
export function ChoiceSkeleton({ count = 5 }: { count?: number }) {
    return <SkeletonRows kind="setting" count={count} label="Loading choices…" />;
}
/** Compact same-scope refresh indicator; never replaces already-valid rows. */
export function SkeletonActivity({ label = "Refreshing…" }: { label?: string }) {
    return <Skeleton className="skeleton-activity" label={label}><Block /></Skeleton>;
}
