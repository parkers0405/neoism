import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { Plus, X } from 'lucide-react';
import type { ChatTab } from '../tabs';
import '../tabs.css';
export function chatTabTitle(tab: ChatTab): string {
    const title = tab.metadata?.title?.trim();
    return title && !/^New session\s*-\s*\d+$/.test(title) ? title : "New tab";
}
export function tabWheelDelta(event: Pick<WheelEvent, 'deltaX' | 'deltaY' | 'deltaMode'>, width: number): number {
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
    return delta * (event.deltaMode === 1 ? 24 : event.deltaMode === 2 ? width : 1);
}
interface TabExit { tab: ChatTab; width: number; successors: string[]; }
function ClosingTab({ exit, done }: { exit: TabExit; done(key: string): void }) {
    useEffect(() => {
        const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
        const finish = () => done(exit.tab.key);
        if (motion.matches) { finish(); return; }
        // Fallback covers missing/cancelled animation events (e.g. a background window).
        const timer = window.setTimeout(finish, 200);
        const changed = () => { if (motion.matches) finish(); };
        motion.addEventListener('change', changed);
        return () => { window.clearTimeout(timer); motion.removeEventListener('change', changed); };
    }, [exit.tab.key, done]);
    return <div className="chat-tab chat-tab-exit" data-exit-key={exit.tab.key} aria-hidden="true" inert
        style={{ '--tab-exit-width': `${exit.width}px` } as CSSProperties}
        onAnimationEnd={event => { if (event.target === event.currentTarget && event.animationName === 'chat-tab-exit') done(exit.tab.key); }}>
        <span className="chat-tab-label">{chatTabTitle(exit.tab)}</span>
    </div>;
}
export function ChatTabs({ tabs, active, activate, close, add }: { tabs: ChatTab[]; active: string; activate(key: string): void; close(key: string): void; add(): void }) {
    const list = useRef<HTMLDivElement>(null);
    const restoreFocus = useRef(false);
    const [exits, setExits] = useState<TabExit[]>([]);
    const finishExit = useCallback((key: string) => setExits(items => items.filter(item => item.tab.key !== key)), []);
    // Place each copy before its next surviving neighbour, including rapid adjacent closes.
    const liveKeys = new Set(tabs.map(tab => tab.key));
    const exitsBefore = (key?: string) => exits.filter(exit => !liveKeys.has(exit.tab.key) && exit.successors.find(next => liveKeys.has(next)) === key)
        .map(exit => <ClosingTab key={`exit-${exit.tab.key}`} exit={exit} done={finishExit} />);
    useEffect(() => {
        // A reopened/replaced key must not retain an exit record whose timer was unmounted.
        setExits(items => items.some(item => tabs.some(tab => tab.key === item.tab.key))
            ? items.filter(item => !tabs.some(tab => tab.key === item.tab.key)) : items);
    }, [tabs]);
    useEffect(() => {
        const selected = list.current?.querySelector<HTMLButtonElement>('[role="tab"][aria-selected="true"]');
        selected?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        if (restoreFocus.current) selected?.focus();
        restoreFocus.current = false;
    }, [active, tabs.length]);
    useEffect(() => {
        const element = list.current;
        if (!element) return;
        const wheel = (event: WheelEvent) => {
            if (event.ctrlKey || element.scrollWidth <= element.clientWidth) return;
            const delta = tabWheelDelta(event, element.clientWidth);
            if (!delta) return;
            event.preventDefault();
            element.scrollLeft = Math.max(0, Math.min(element.scrollWidth - element.clientWidth, element.scrollLeft + delta));
        };
        element.addEventListener('wheel', wheel, { passive: false });
        return () => element.removeEventListener('wheel', wheel);
    }, []);
    const closeView = (key: string) => {
        const index = tabs.findIndex(tab => tab.key === key);
        const node = list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[index]?.parentElement;
        if (index >= 0 && node && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
            const width = node.getBoundingClientRect().width;
            const siblings = Array.from(list.current!.children);
            const nextExit = siblings.slice(siblings.indexOf(node) + 1).find(sibling => sibling.hasAttribute('data-exit-key'))?.getAttribute('data-exit-key');
            setExits(items => {
                if (items.some(item => item.tab.key === key)) return items;
                const next = [...items], before = items.findIndex(item => item.tab.key === nextExit);
                next.splice(before < 0 ? next.length : before, 0, { tab: tabs[index], width, successors: tabs.slice(index + 1).map(tab => tab.key) });
                return next;
            });
        }
        restoreFocus.current = true;
        close(key);
    };
    return <div className="chat-tabs-bar">
        <div ref={list} className="chat-tabs" role="tablist" aria-label="Open chats">
            {[...tabs.flatMap((tab, index) => [...exitsBefore(tab.key), <div className={`chat-tab ${tab.key === active ? 'selected' : ''}`} key={tab.key}>
                <button type="button" role="tab" id={`chat-tab-${tab.key}`} aria-controls={tab.key === active ? `chat-panel-${tab.key}` : undefined}
                    aria-selected={tab.key === active} tabIndex={tab.key === active ? 0 : -1}
                    onClick={() => activate(tab.key)} title={chatTabTitle(tab)}
                    onKeyDown={(event) => {
                        let next: number;
                        if (event.key === 'ArrowRight') next = (index + 1) % tabs.length;
                        else if (event.key === 'ArrowLeft') next = (index - 1 + tabs.length) % tabs.length;
                        else if (event.key === 'Home') next = 0;
                        else if (event.key === 'End') next = tabs.length - 1;
                        else if (event.key === 'Delete') { event.preventDefault(); closeView(tab.key); return; }
                        else return;
                        event.preventDefault();
                        list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
                        activate(tabs[next].key);
                    }}><span className="chat-tab-label">{chatTabTitle(tab)}</span></button>
                <button type="button" className="chat-tab-close" tabIndex={tab.key === active ? 0 : -1}
                    aria-label={`Close ${chatTabTitle(tab)}`} title="Close tab (keeps chat)" onClick={() => closeView(tab.key)}><X size={13} /></button>
            </div>]), ...exitsBefore()]}
        </div>
        <button type="button" className="chat-tab-add" aria-label="New chat" title="New chat" onClick={add}><Plus size={17} /></button>
    </div>;
}
