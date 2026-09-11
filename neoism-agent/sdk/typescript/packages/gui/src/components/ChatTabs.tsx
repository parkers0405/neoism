import { useEffect, useRef } from 'react';
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
export function ChatTabs({ tabs, active, activate, close, add }: { tabs: ChatTab[]; active: string; activate(key: string): void; close(key: string): void; add(): void }) {
    const list = useRef<HTMLDivElement>(null);
    const restoreFocus = useRef(false);
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
        restoreFocus.current = true;
        close(key);
    };
    return <div className="chat-tabs-bar">
        <div ref={list} className="chat-tabs" role="tablist" aria-label="Open chats">
            {tabs.map((tab, index) => <div className={`chat-tab ${tab.key === active ? 'selected' : ''}`} key={tab.key}>
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
            </div>)}
        </div>
        <button type="button" className="chat-tab-add" aria-label="New chat" title="New chat" onClick={add}><Plus size={17} /></button>
    </div>;
}
