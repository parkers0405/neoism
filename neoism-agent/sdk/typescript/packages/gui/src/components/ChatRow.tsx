import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, Ellipsis, Pencil, Pin, Trash2, X } from "lucide-react";
import type { Session } from "@neoism/sdk";
import { isSessionPinned } from "../sessionPins";
import "./chat-menu.css";

export function ChatRow({ session: s, selected, open, pin, rename, remove }: {
    session: Session; selected: boolean;
    open: () => void; pin: (pinned: boolean) => Promise<void>;
    rename: (title: string) => Promise<void>; remove: () => Promise<void>;
}) {
    const [anchor, setAnchor] = useState<{ top: number; right: number }>();
    const [editing, setEditing] = useState(false);
    const [title, setTitle] = useState(s.title);
    const [busy, setBusy] = useState(false);
    const trigger = useRef<HTMLButtonElement>(null);
    const menu = useRef<HTMLDivElement>(null);
    const input = useRef<HTMLInputElement>(null);
    const menuId = useId();
    const pinned = isSessionPinned(s);
    const label = s.title || "Untitled chat";
    const close = (restore = true) => { setAnchor(undefined); if (restore) trigger.current?.focus(); };
    useEffect(() => {
        if (!anchor) return;
        menu.current?.querySelector<HTMLButtonElement>("button")?.focus();
        const outside = (event: PointerEvent) => {
            if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) close(false);
        };
        const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) close(false); };
        const resize = () => close(false);
        document.addEventListener("pointerdown", outside);
        document.addEventListener("scroll", scroll, true);
        window.addEventListener("resize", resize);
        return () => {
            document.removeEventListener("pointerdown", outside);
            document.removeEventListener("scroll", scroll, true);
            window.removeEventListener("resize", resize);
        };
    }, [anchor]);
    useEffect(() => { if (editing) { input.current?.focus(); input.current?.select(); } }, [editing]);
    const togglePin = async () => { setBusy(true); try { await pin(!pinned); } finally { setBusy(false); } };
    const cancelRename = () => { setEditing(false); trigger.current?.focus(); };
    const save = async () => {
        if (!title.trim() || busy) return;
        setBusy(true);
        try { await rename(title.trim()); cancelRename(); } finally { setBusy(false); }
    };
    return <div className={`recent${selected ? " selected" : ""}`}>
        {editing ? <form className="chat-rename" onSubmit={e => { e.preventDefault(); void save(); }}>
            <input ref={input} aria-label="Chat name" value={title} disabled={busy}
                onChange={e => setTitle(e.target.value)} onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); cancelRename(); } }} />
            <button type="submit" aria-label="Save chat name" disabled={busy || !title.trim()}><Check size={14} /></button>
            <button type="button" aria-label="Cancel rename" onClick={cancelRename}><X size={14} /></button>
        </form> : <button className="recent-title" title={s.title} onClick={open}>{label}</button>}
        <button className={`recent-action chat-pin${pinned ? " is-pinned" : ""}`} disabled={busy}
            aria-label={`${pinned ? "Unpin" : "Pin"} ${label}`} aria-pressed={pinned} onClick={() => void togglePin()}><Pin size={15} /></button>
        <button ref={trigger} className="recent-action chat-menu-trigger" aria-label={`Actions for ${label}`}
            aria-haspopup="menu" aria-expanded={!!anchor} aria-controls={anchor ? menuId : undefined}
            onClick={() => {
                if (anchor) { close(); return; }
                const rect = trigger.current!.getBoundingClientRect();
                setAnchor({ top: Math.max(8, Math.min(rect.bottom + 4, window.innerHeight - 96)), right: Math.max(8, window.innerWidth - rect.right) });
            }}><Ellipsis size={16} /></button>
        {anchor && createPortal(<div className="chat-actions-menu" id={menuId} ref={menu} role="menu" aria-label={`Actions for ${label}`}
            style={{ top: anchor.top, right: anchor.right }} onBlur={event => {
                if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) close(false);
            }} onKeyDown={event => {
                if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
                if (event.key === "Tab") { close(false); return; }
                const items = [...menu.current!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
                const i = items.indexOf(document.activeElement as HTMLButtonElement);
                const index = event.key === "ArrowDown" ? (i + 1) % items.length : event.key === "ArrowUp" ? (i - 1 + items.length) % items.length : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : -1;
                if (index >= 0) { event.preventDefault(); items[index]?.focus(); }
            }}>
            <button role="menuitem" onClick={() => { close(); setTitle(s.title); setEditing(true); }}><Pencil size={14} />Rename</button>
            <button role="menuitem" onClick={() => { close(); if (confirm(`Delete “${label}”? This cannot be undone.`)) void remove(); }}><Trash2 size={14} />Delete</button>
        </div>, document.body)}
    </div>;
}
