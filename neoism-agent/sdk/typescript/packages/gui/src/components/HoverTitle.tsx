import type { CSSProperties } from "react";
import "./hover-title.css";

/** Reveal overflow without widening the row or covering adjacent controls. */
export function HoverTitle({ text }: { text: string }) {
    return <span className="hover-title" onMouseEnter={event => {
        const viewport = event.currentTarget;
        const content = viewport.firstElementChild as HTMLElement;
        const distance = Math.max(0, content.scrollWidth - viewport.clientWidth);
        viewport.style.setProperty("--title-travel", `${-distance}px`);
        viewport.style.setProperty("--title-duration", `${Math.max(2, distance / 35)}s`);
        viewport.dataset.overflow = String(distance > 0);
    }} style={{ "--title-travel": "0px" } as CSSProperties}>
        <span className="hover-title-text">{text}</span>
    </span>;
}
