// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Avatar } from "./Identity";
import { UserAvatar } from "./UserAvatar";
import { Navigation } from "./Navigation";
import { Timeline } from "./Timeline";
import { avatarCells } from "../generated/avatar";
import { resolveIdentity, messageAuthor } from "../identity";

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
describe("profile identity", () => {
    it("uses explicit name, server native override, then server OS identity, never browser host", () => {
        expect(resolveIdentity(" Fern ", { configuredName: "Native", systemName: "os-user" })).toBe("Fern");
        expect(resolveIdentity("You", { configuredName: "Native", systemName: "os-user" })).toBe("Native");
        expect(resolveIdentity(" ", { configuredName: " ", systemName: " os-user " })).toBe("os-user");
        expect(resolveIdentity("", {})).toBe("You");
        expect(resolveIdentity("You")).toBe("You");
    });
    it("sidebar uses the resolved profile rather than persisted You and shares the message avatar", () => {
        const app = { prefs: { name: "You" }, identityName: "Native Name", sessions: [], search: "" } as unknown as Parameters<typeof Navigation>[0]["app"];
        const sidebar = renderToStaticMarkup(<Navigation app={app} />);
        const avatar = renderToStaticMarkup(<Avatar seed="Native Name" />);
        expect(sidebar).toContain('<strong>Native Name</strong>');
        expect(sidebar).toContain(avatar);
        expect(renderToStaticMarkup(<UserAvatar info={{}} localName={app.identityName} />)).toContain(avatar);
    });
    it("retains remote authors and falls back only for unattributed messages", () => {
        expect(messageAuthor({ author: " Remote Person " }, "Local")).toBe("Remote Person");
        expect(messageAuthor({ author: "You" }, "Local")).toBe("You");
        expect(messageAuthor({ author: 42 }, "Local")).toBe("Anonymous user");
        expect(messageAuthor({ author: null }, "You")).toBe("Anonymous user");
        expect(messageAuthor({ author: " " }, "You")).toBe("Anonymous user");
        expect(messageAuthor({}, "You")).toBe("You");
        expect(messageAuthor({ author: "Remote" }, "You")).toBe("Remote");
        expect(messageAuthor({}, "Local")).toBe("Local");
        const html = renderToStaticMarkup(<UserAvatar info={{ author: "Remote Person" }} localName="Local" />);
        expect(html).toContain('role="tooltip">Remote Person');
        expect(html).toContain('tabindex="0"');
        expect(html).toContain("Remote Person&#x27;s avatar");
        expect(html).not.toContain("Local");
    });
    it("renders the same avatar at the right of human messages, not assistant/runtime cards", () => {
        const messages = [
            { info: { id: "u", sessionId: "s", role: "user", author: "Remote" }, parts: [{ id: "p", type: "text", text: "hello" }] },
            { info: { id: "a", sessionId: "s", role: "assistant" }, parts: [{ id: "a-p", type: "text", text: "reply" }] },
            { info: { id: "msg_background_completion_job", sessionId: "s", role: "user" }, parts: [{ id: "r", type: "text", text: "done" }] },
        ] as any;
        const html = renderToStaticMarkup(<Timeline localName="Local" messages={messages} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html.match(/class="user-message-avatar"/g)).toHaveLength(1);
        expect(html).toContain(renderToStaticMarkup(<Avatar seed="Remote" />));
    });
});

it("advances native plasma fills, synchronizes copies, honors live reduced motion and cleans up", () => {
    const media = new EventTarget() as EventTarget & { matches: boolean };
    media.matches = false;
    vi.spyOn(window, "matchMedia").mockReturnValue(media as MediaQueryList);
    let hidden = false;
    vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
    const observers: ((entries: { isIntersecting: boolean }[]) => void)[] = [];
    const disconnect = vi.fn();
    vi.stubGlobal("IntersectionObserver", class {
        constructor(callback: (entries: { isIntersecting: boolean }[]) => void) { observers.push(callback); }
        observe() {}
        disconnect = disconnect;
    });
    const frames = new Map<number, FrameRequestCallback>(); let serial = 0;
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { frames.set(++serial, cb); return serial; });
    vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host);
    const fills = (index = 0) => [...host.querySelectorAll('svg')[index].querySelectorAll('rect')].map(r => r.getAttribute('fill'));
    try {
        act(() => root.render(<><Avatar seed="Fern" /><Avatar seed="Fern" /></>));
        const advance = (time: number) => { const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(cb => cb(time)); };
        act(() => advance(2000));
        expect(fills()).toEqual(avatarCells("Fern", 2).map(c => c.color));
        expect(fills(1)).toEqual(fills());
        const prior = fills(); act(() => advance(3000)); expect(fills()).not.toEqual(prior);
        act(() => { media.matches = true; media.dispatchEvent(new Event("change")); });
        expect(frames.size).toBe(0);
        expect(fills()).toEqual(avatarCells("Fern", .6).map(c => c.color));
        act(() => { media.matches = false; media.dispatchEvent(new Event("change")); });
        expect(frames.size).toBe(2);
        act(() => observers.forEach(cb => cb([{ isIntersecting: false }])));
        expect(frames.size).toBe(0);
        act(() => observers.forEach(cb => cb([{ isIntersecting: true }])));
        expect(frames.size).toBe(2);
        act(() => { hidden = true; document.dispatchEvent(new Event('visibilitychange')); });
        expect(frames.size).toBe(0);
        act(() => { hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
        expect(frames.size).toBe(2);
        act(() => root.render(<Avatar seed="Other" />));
        act(() => advance(4000));
        expect(fills()).toEqual(avatarCells("Other", 4).map(c => c.color));
    } finally { act(() => root.unmount()); host.remove(); }
    expect(frames.size).toBe(0);
});
