import { useEffect, useRef, useState } from "react";
import {
    Plus,
    Sparkles,
    GitBranch,
    Search,
    Settings,
    ArrowDown,
} from "lucide-react";
import { ChatRow } from "./ChatRow";
import { SkeletonRows } from "./Skeleton";
import { Avatar } from "./Identity";
import { isSessionPinned } from "../sessionPins";
import type { Session } from "@neoism/sdk";
import type { useAppController } from "../useAppController";

const PINNED_COLLAPSED_KEY = "neoism.gui.pinned-collapsed";
function readPinnedCollapsed() {
    try { return localStorage.getItem(PINNED_COLLAPSED_KEY) === "1"; } catch { return false; }
}
export function Navigation({
    app: a,
}: {
    app: ReturnType<typeof useAppController> & { loading?: boolean };
}) {
    // Controller's initial/replacement loading flag is distinct from pagination.
    const initialLoading = a.loading ?? (a.listBusy && !a.sessions.length);
    const scroller = useRef<HTMLDivElement>(null);
    const sentinel = useRef<HTMLDivElement>(null);
    const requested = useRef<{client: typeof a.client; scope: string; cursor: string} | undefined>(undefined);
    const [pinnedCollapsed, setPinnedCollapsed] = useState(readPinnedCollapsed);
    const scope = `${a.prefs.server}:${a.prefs.directory}:${a.search}`;
    const chats = a.sessions.filter((s) => !s.parentId);
    const pinned = chats.filter(isSessionPinned);
    const recents = chats.filter((s) => !isSessionPinned(s));
    const row = (s: Session) => (
        <ChatRow key={`${scope}:${s.id}`} session={s}
            selected={s.id === a.id && a.view === "chat"}
            open={() => a.openSession(s.id)} pin={next => a.pinSession(s.id, next)}
            rename={title => a.renameSession(s.id, title)} remove={() => a.deleteSession(s.id)} />
    );
    const loadMore = () => {
        if (initialLoading || a.listBusy || !a.cursor) return;
        const last = requested.current;
        if (last?.client === a.client && last.scope === scope && last.cursor === a.cursor) return;
        requested.current = {client:a.client,scope,cursor:a.cursor};
        void a.recentMore();
    };
    useEffect(() => {
        const root = scroller.current, target = sentinel.current;
        if (!root || !target || initialLoading || a.listBusy || typeof IntersectionObserver === "undefined") return;
        let active = true;
        const observer = new IntersectionObserver(entries => {
            if (active && entries.some(entry => entry.isIntersecting)) loadMore();
        }, {root,rootMargin:"120px 0px"});
        observer.observe(target);
        return () => { active = false; observer.disconnect(); };
    }, [a.client, scope, a.cursor, a.listBusy, initialLoading]);
    return (
        <aside className="left-nav" id="app-navigation" aria-label="Navigation">
            <nav>
                <button onClick={a.newChat}>
                    <Plus size={16} />
                    New Chat
                </button>
                <button
                    className={a.view === "skills" ? "selected" : ""}
                    onClick={() => {
                        a.setView("skills");
                        a.setNav(false);
                    }}
                >
                    <Sparkles size={16} />
                    Skills
                </button>
                <button
                    className={a.view === "workflows" ? "selected" : ""}
                    onClick={() => {
                        a.setView("workflows");
                        a.setNav(false);
                    }}
                >
                    <GitBranch size={16} />
                    Workflows
                </button>
            </nav>
            <label className="search">
                <Search size={16} />
                <input
                    aria-label="Search chats"
                    placeholder="Search chats"
                    value={a.search}
                    onChange={(e) => a.setSearch(e.target.value)}
                />
            </label>
            <div className="recents" ref={scroller} onScroll={event => {
                const node = event.currentTarget;
                if (node.scrollHeight - node.scrollTop - node.clientHeight < 120) loadMore();
            }}>
                {initialLoading ? <SkeletonRows kind="session" count={6} header /> : <>
                    {pinned.length > 0 && <section className="session-group">
                        <button type="button" className="session-group-heading" aria-expanded={!pinnedCollapsed}
                            aria-controls="pinned-chats" onClick={() => setPinnedCollapsed(collapsed => {
                                const next = !collapsed;
                                try { localStorage.setItem(PINNED_COLLAPSED_KEY, next ? "1" : "0"); } catch { /* collapse is presentation-only */ }
                                return next;
                            })}>
                            Pinned
                        </button>
                        {!pinnedCollapsed && <div id="pinned-chats">{pinned.map(row)}</div>}
                    </section>}
                    {(recents.length > 0 || pinned.length > 0) && <div className="session-group-heading recents-label">Recents</div>}
                    {recents.map(row)}
                </>}
                {!initialLoading && a.cursor && <div ref={sentinel} className="recents-sentinel" aria-hidden="true" style={{height:1}} />}
                {!initialLoading && a.listBusy && <SkeletonRows kind="session" count={3} label="Loading more chats…" />}
                {!initialLoading && !a.listBusy && a.cursor && (
                    <button
                        type="button"
                        className="history-page-control"
                        aria-label="Load more conversations"
                        onClick={() => { requested.current = undefined; loadMore(); }}
                    >
                        <ArrowDown size={15} aria-hidden="true" />
                    </button>
                )}
                {!initialLoading && !a.sessions.length && !a.listBusy && (
                    <p className="muted">
                        Your conversations will appear here.
                    </p>
                )}
            </div>
            <button className="profile" onClick={() => a.setSettings(true)}>
                <Avatar seed={a.prefs.name} />
                <span>
                    <strong>{a.prefs.name || "You"}</strong>
                </span>
                <Settings size={16} />
            </button>
        </aside>
    );
}
