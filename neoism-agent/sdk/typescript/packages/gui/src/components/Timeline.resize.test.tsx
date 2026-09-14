// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { Timeline } from "./Timeline";

it("preserves a reader's message offset during panel-width reflow and still follows the tail", () => {
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    let resize = () => {}, width = 800, messageTop = 10;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
        constructor(callback: () => void) { resize = callback; }
        observe() {}
        disconnect = disconnect;
    });
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => width);
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(2000);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
        const top = this.hasAttribute('data-message-id') ? messageTop : 0;
        return {top, bottom: top + 100, width, height: 100, x: 0, y: top, left: 0, right: width, toJSON() {}};
    });
    const host = document.createElement('div'); document.body.append(host);
    const root = createRoot(host);
    try {
        const messages = [{info: {id: 'm', sessionId: 's', role: 'user'}, parts: [{id: 'p', type: 'text', text: 'Message'}]}] as any;
        act(() => root.render(<Timeline messages={messages} busy={false} loading={false} loadOlder={() => {}} showActivity={false} />));
        const timeline = host.querySelector<HTMLElement>('.timeline')!;
        act(() => { timeline.scrollTop = 500; timeline.dispatchEvent(new Event('scroll')); });
        width = 560; messageTop = 70;
        act(() => resize());
        expect(timeline.scrollTop).toBe(560);
        // Same-width output updates must not force a scrolled-up reader to the bottom.
        messageTop = 90;
        act(() => resize());
        expect(timeline.scrollTop).toBe(560);
        act(() => { timeline.scrollTop = 1500; timeline.dispatchEvent(new Event('scroll')); });
        width = 800;
        act(() => resize());
        expect(timeline.scrollTop).toBe(2000);
    } finally {
        act(() => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals();
    }
    expect(disconnect).toHaveBeenCalled();
});
