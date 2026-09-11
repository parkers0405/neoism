import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { filterChoices, indexChoices, layoutChoices, visibleChoices, revealChoice, PICKER_HEIGHT } from "../pickerState";
import { ComposerPanel } from "./ComposerPanel";
import { ChoiceSkeleton } from "./Skeleton";
import { moveSelection } from "../commands";
import type { Choice } from "./Composer";

export function Picker({ title, choices, choose, close, onSearch, loading = false }: {
    title: string; choices: Choice[]; choose(id: string): void; close(): void;
    onSearch?(query: string): void; loading?: boolean;
}) {
    const [query, setQuery] = useState("");
    const [index, setIndex] = useState(0);
    const [top, setTop] = useState(0);
    const [viewport, setViewport] = useState(PICKER_HEIGHT);
    const list = useRef<HTMLDivElement>(null);
    const searchIndex = useMemo(() => indexChoices(choices), [choices]);
    const filtered = useMemo(() => filterChoices(searchIndex, query), [searchIndex, query]);
    const layout = useMemo(() => layoutChoices(filtered), [filtered]);
    const selected = Math.min(index, Math.max(0, filtered.length - 1));
    const rows = visibleChoices(layout, top, viewport);
    useEffect(() => {
        const node = list.current;
        if (!node || typeof ResizeObserver === 'undefined') return;
        const measure = () => setViewport(node.clientHeight || PICKER_HEIGHT);
        measure(); const observer = new ResizeObserver(measure); observer.observe(node);
        return () => observer.disconnect();
    }, []);
    useLayoutEffect(() => {
        const node = list.current;
        if (!node) return;
        const next = revealChoice(layout, selected, node.scrollTop, node.clientHeight || viewport);
        node.scrollTop = next; setTop(next);
        // Scroll the logical option into the viewport, even when it wasn't mounted.
        // Wheel scrolling doesn't change selection or snap back to the selected row.
    }, [selected, layout, viewport]);
    return <ComposerPanel title={title} close={close}>
        <input autoFocus aria-label={`Search ${title}`} placeholder="Search…" value={query}
            onChange={e => {
                setQuery(e.target.value); onSearch?.(e.target.value); setIndex(0); setTop(0);
                if (list.current) list.current.scrollTop = 0;
            }}
            onKeyDown={e => {
                if (!["ArrowUp", "ArrowDown", "Enter", "Tab"].includes(e.key)) return;
                e.preventDefault();
                if (e.key === "Enter") { if (filtered[selected]) choose(filtered[selected].id); }
                else setIndex(moveSelection(selected, e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey) ? -1 : 1, filtered.length));
            }} />
        <div className="choice-list" role="listbox" ref={list}
            style={{ height: Math.min(PICKER_HEIGHT, layout.height || 48), maxHeight: 'min(336px, 40vh)', overflowY: 'auto', position: 'relative', display: 'block', overflowAnchor: 'none' }}
            onScroll={e => setTop(e.currentTarget.scrollTop)}>
            <div role="presentation" style={{ height: layout.height, position: 'relative' }}>
                {rows.map(row => row.choice ? <button key={`option-${row.index}`} type="button" role="option"
                    aria-posinset={row.index + 1} aria-setsize={filtered.length} aria-selected={row.index === selected}
                    style={{ position: 'absolute', top: row.top, left: 0, width: '100%', height: row.height, boxSizing: 'border-box', overflow: 'hidden', margin: 0 }}
                    onClick={() => choose(row.choice!.id)}>
                    <strong style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.choice.label}</strong>
                    <small style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.choice.description || row.choice.id}{row.choice.badge && <span> · {row.choice.badge}</span>}</small>
                </button> : <div key={`header-${row.top}`} role="presentation" className="choice-section"
                    style={{ position: 'absolute', top: row.top, left: 0, height: row.height, lineHeight: `${row.height}px`, overflow: 'hidden' }}>{row.section}</div>)}
            </div>
            {!filtered.length && !loading && <p className="empty">No matches. Check the server's configured providers.</p>}
        </div>
        {loading && <ChoiceSkeleton count={3} />}
    </ComposerPanel>;
}
