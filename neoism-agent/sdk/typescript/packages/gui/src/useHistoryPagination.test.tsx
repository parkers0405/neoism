// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useHistoryPagination } from "./useHistoryPagination";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement, root: Root;
function View(props: Parameters<typeof useHistoryPagination>[0]) {
    return <div data-viewport tabIndex={0} {...useHistoryPagination(props)}><pre data-code style={{overflowY:"auto"}}>code</pre></div>;
}
async function render(props: Parameters<typeof useHistoryPagination>[0]) {
    await act(async () => root.render(<View {...props} />));
    return host.querySelector<HTMLDivElement>("[data-viewport]")!;
}
async function wheel(element: HTMLElement, deltaY = -40) {
    await act(async () => element.dispatchEvent(new WheelEvent("wheel", {deltaY,bubbles:true,cancelable:true})));
}
beforeEach(() => { host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });

describe("upward history pagination", () => {
    it("lets expanded tool content consume scrolling before paging the conversation", async () => {
        const loadOlder = vi.fn(async () => {});
        const element = await render({sessionId:"s",cursor:"old",loading:false,loadOlder});
        const code = element.querySelector("pre")!;
        Object.defineProperties(code, {scrollHeight:{value:600,configurable:true},clientHeight:{value:100,configurable:true}});
        code.scrollTop = 100;
        await wheel(code);
        expect(loadOlder).not.toHaveBeenCalled();
        code.scrollTop = 0;
        await wheel(code);
        expect(loadOlder).toHaveBeenCalledTimes(1);
    });
    it("does not load on mount or programmatic scroll, only user intent near the top", async () => {
        const loadOlder = vi.fn(async () => {});
        const element = await render({sessionId:"s",cursor:"older",loading:false,loadOlder});
        await act(async () => element.dispatchEvent(new Event("scroll")));
        expect(loadOlder).not.toHaveBeenCalled();
        element.scrollTop = 300; await wheel(element); expect(loadOlder).not.toHaveBeenCalled();
        element.scrollTop = 50;
        await act(async () => element.dispatchEvent(new Event("scroll")));
        expect(loadOlder).toHaveBeenCalledTimes(1);
    });
    it("requests a cursor only once and never cascades after a prepend", async () => {
        const loadOlder = vi.fn(async () => {});
        let element = await render({sessionId:"s",cursor:"one",loading:false,loadOlder});
        await wheel(element); await wheel(element); expect(loadOlder).toHaveBeenCalledTimes(1);
        element = await render({sessionId:"s",cursor:"two",loading:false,loadOlder});
        await act(async () => element.dispatchEvent(new Event("scroll")));
        expect(loadOlder).toHaveBeenCalledTimes(1);
        await wheel(element); expect(loadOlder).toHaveBeenCalledTimes(2);
    });
    it("ignores downward input, pending loads and exhausted history", async () => {
        const loadOlder = vi.fn(async () => {});
        let element = await render({sessionId:"s",cursor:"one",loading:false,loadOlder});
        await wheel(element,40);
        element = await render({sessionId:"s",cursor:"one",loading:true,loadOlder}); await wheel(element);
        element = await render({sessionId:"s",loading:false,loadOlder}); await wheel(element);
        expect(loadOlder).not.toHaveBeenCalled();
    });
    it("resets request ownership when switching sessions", async () => {
        let resolve!: () => void;
        const loadOlder = vi.fn().mockImplementationOnce(() => new Promise<void>(r => {resolve=r;})).mockResolvedValue(undefined);
        let element = await render({sessionId:"a",cursor:"one",loading:false,loadOlder}); await wheel(element);
        element = await render({sessionId:"b",cursor:"one",loading:false,loadOlder}); await wheel(element);
        expect(loadOlder).toHaveBeenCalledTimes(2);
        await act(async () => resolve());
        await wheel(element); expect(loadOlder).toHaveBeenCalledTimes(2);
    });
    it("accepts keyboard and touch navigation without preventing native touch scrolling", async () => {
        const loadOlder = vi.fn(async () => {});
        let element = await render({sessionId:"a",cursor:"one",loading:false,loadOlder});
        await act(async () => element.dispatchEvent(new KeyboardEvent("keydown",{key:"PageUp",bubbles:true})));
        expect(loadOlder).toHaveBeenCalledTimes(1);
        element = await render({sessionId:"b",cursor:"one",loading:false,loadOlder});
        const start = Object.assign(new Event("touchstart",{bubbles:true,cancelable:true}),{touches:[{clientY:100}]});
        const move = Object.assign(new Event("touchmove",{bubbles:true,cancelable:true}),{touches:[{clientY:140}]});
        await act(async () => {element.dispatchEvent(start);element.dispatchEvent(move);});
        expect(loadOlder).toHaveBeenCalledTimes(2);
        expect(move.defaultPrevented).toBe(false);
    });
});
