// @vitest-environment happy-dom
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import type { Session } from "@neoism/sdk";
import { ChatTabs, chatTabTitle, tabWheelDelta } from "./ChatTabs";
import type { ChatTab } from "../tabs";

const tab = (key: string, draft = ""): ChatTab => ({key, draft, explicit:{}});
const tabStyles = readFileSync("src/tabs.css", "utf8");
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it.each([false, true])("closes views immediately with bounded inert exits (reduced motion: %s)", reduced => {
    vi.useFakeTimers();
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    const motion = Object.assign(new EventTarget(), { matches: reduced });
    vi.stubGlobal("matchMedia", () => motion);
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host), close = vi.fn();
    function Tabs() {
        const [tabs, setTabs] = useState([tab("a"), tab("b"), tab("c"), tab("d")]);
        const [active, activate] = useState("a");
        return <ChatTabs tabs={tabs} active={active} activate={activate} add={() => {}} close={key => {
            close(key);
            const next = tabs.filter(tab => tab.key !== key);
            setTabs(next); activate(next[0].key);
        }} />;
    }
    try {
        act(() => root.render(<Tabs />));
        host.querySelector<HTMLButtonElement>('[role="tab"]')!.focus();
        const remove = () => act(() => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true })));
        remove();
        expect(close).toHaveBeenLastCalledWith("a");
        expect(document.activeElement?.id).toBe("chat-tab-b");
        expect(host.querySelectorAll('[role="tab"]')).toHaveLength(3);
        act(() => vi.advanceTimersByTime(50));
        remove();
        expect(close).toHaveBeenLastCalledWith("b");
        expect(document.activeElement?.id).toBe("chat-tab-c");
        expect(host.querySelectorAll('[role="tab"]')).toHaveLength(2);
        const copies = host.querySelectorAll('.chat-tab-exit');
        expect(copies).toHaveLength(reduced ? 0 : 2);
        for (const copy of copies) {
            expect(copy.hasAttribute('inert')).toBe(true);
            expect(copy.getAttribute('aria-hidden')).toBe('true');
            expect(copy.querySelector('button, [role="tab"]')).toBeNull();
        }
        if (!reduced) {
            // Each exit keeps its own deadline when its next neighbour also closes.
            act(() => vi.advanceTimersByTime(151));
            expect(host.querySelectorAll('.chat-tab-exit')).toHaveLength(1);
            const exit = host.querySelector('.chat-tab-exit')!;
            act(() => exit.dispatchEvent(Object.assign(new Event('animationend', {bubbles: true}), {animationName: 'chat-tab-exit'})));
            expect(host.querySelector('.chat-tab-exit')).toBeNull();
            expect(vi.getTimerCount()).toBe(0);
            remove();
            expect(host.querySelector('.chat-tab-exit')).not.toBeNull();
            act(() => { motion.matches = true; motion.dispatchEvent(new Event('change')); });
            expect(host.querySelector('.chat-tab-exit')).toBeNull();
            expect(vi.getTimerCount()).toBe(0);
        }
    } finally { act(() => root.unmount()); host.remove(); }
    expect(vi.getTimerCount()).toBe(0);
});
it("keeps reverse-order closes in place and clears exits when keys reopen or the tab bar unmounts", () => {
    vi.useFakeTimers();
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal("matchMedia", () => Object.assign(new EventTarget(), {matches: false}));
    const host = document.createElement('div'); document.body.append(host);
    const root = createRoot(host);
    let tabs = [tab('a'), tab('b'), tab('c')];
    const render = () => root.render(<ChatTabs tabs={tabs} active="c" activate={() => {}} add={() => {}} close={key => {
        tabs = tabs.filter(tab => tab.key !== key); render();
    }} />);
    try {
        act(render);
        act(() => host.querySelector<HTMLButtonElement>('#chat-tab-b + button')!.click());
        act(() => host.querySelector<HTMLButtonElement>('#chat-tab-a + button')!.click());
        expect([...host.querySelectorAll('.chat-tab-exit')].map(node => node.getAttribute('data-exit-key'))).toEqual(['a', 'b']);
        act(() => { tabs = [tab('a'), tab('b'), tab('c')]; render(); });
        expect(host.querySelector('.chat-tab-exit')).toBeNull();
        expect(vi.getTimerCount()).toBe(0);
        act(() => host.querySelector<HTMLButtonElement>('#chat-tab-a + button')!.click());
        expect(host.querySelectorAll('.chat-tab-exit')).toHaveLength(1);
        expect(vi.getTimerCount()).toBe(1);
    } finally { act(() => root.unmount()); host.remove(); }
    expect(vi.getTimerCount()).toBe(0);
});
it("scopes width/fade motion to exit copies with a reduced-motion override", () => {
    expect(tabStyles).toContain('animation: chat-tab-exit 160ms');
    expect(tabStyles).toContain('to { width: 0; opacity: 0; }');
    expect(tabStyles).toContain('@media (prefers-reduced-motion: reduce)');
    expect(tabStyles).toContain('animation: none; width: 0; opacity: 0;');
});
describe("chat tab presentation", () => {
    it("maps vertical wheels and horizontal trackpads into horizontal tab scrolling", () => {
        expect(tabWheelDelta({deltaX:0,deltaY:40,deltaMode:0},300)).toBe(40);
        expect(tabWheelDelta({deltaX:-60,deltaY:5,deltaMode:0},300)).toBe(-60);
        expect(tabWheelDelta({deltaX:0,deltaY:3,deltaMode:1},300)).toBe(72);
        expect(tabWheelDelta({deltaX:0,deltaY:-1,deltaMode:2},300)).toBe(-300);
    });
    it("keeps New tab until the session has a real title, ignoring draft and seed timestamps", () => {
        expect(chatTabTitle(tab("a"))).toBe("New tab");
        expect(chatTabTitle({...tab("a", "Review the parser\nMore details"), metadata:{title:"New session - 1789016003938"} as Session})).toBe("New tab");
        expect(chatTabTitle({...tab("a"), metadata:{title:"Parser fixes"} as Session})).toBe("Parser fixes");
    });
    it("exposes the selected tab and its panel without making every tab a tab stop", () => {
        const html = renderToStaticMarkup(<ChatTabs tabs={[tab("a"),tab("b")]} active="b" activate={vi.fn()} close={vi.fn()} add={vi.fn()} />);
        expect(html).toContain('role="tablist"');
        expect(html).toContain('id="chat-tab-b" aria-controls="chat-panel-b" aria-selected="true" tabindex="0"');
        expect(html).toContain('id="chat-tab-a" aria-selected="false" tabindex="-1"');
        expect(html).toContain("Close tab (keeps chat)");
        expect(html).toContain('<span class="chat-tab-label">New tab</span></button>');
    });
});
