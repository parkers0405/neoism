// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Navigation } from "./Navigation";
import { defaultPreferences } from "../types";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
const observed: {callback: IntersectionObserverCallback; options?: IntersectionObserverInit}[] = [];
let host: HTMLDivElement, root: Root;
const base = () => ({client:{},prefs:defaultPreferences,sessions:[{id:"one",title:"One"}],search:"",loading:false,listBusy:false,cursor:"next",view:"chat",id:undefined,
    newChat:vi.fn(),setView:vi.fn(),setNav:vi.fn(),setSearch:vi.fn(),openSession:vi.fn(),pinSession:vi.fn(async () => {}),renameSession:vi.fn(),deleteSession:vi.fn(),setSettings:vi.fn(),recentMore:vi.fn(async () => {})});
async function render(app: ReturnType<typeof base>) {
    await act(async () => root.render(<Navigation app={app as unknown as Parameters<typeof Navigation>[0]["app"]} />));
}
beforeEach(() => {
    observed.length = 0;
    vi.stubGlobal("IntersectionObserver", class {
        constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) { observed.push({callback,options}); }
        observe() {} disconnect() {}
    });
    host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
const intersect = async (entry: typeof observed[number]) => {
    await act(async () => entry.callback([{isIntersecting:true} as IntersectionObserverEntry], {} as IntersectionObserver));
};

describe("Recents infinite pagination", () => {
    it("loads at the sidebar sentinel and guards repeated observer notifications", async () => {
        const app = base(); await render(app);
        const observer = observed.find(entry => entry.options?.root === host.querySelector(".recents"))!;
        expect(observer.options?.rootMargin).toBe("120px 0px");
        await intersect(observer); await intersect(observer);
        expect(app.recentMore).toHaveBeenCalledTimes(1);
        await render({...app,cursor:"next-page"});
        await intersect(observed.filter(entry => entry.options?.root).at(-1)!);
        expect(app.recentMore).toHaveBeenCalledTimes(2);
    });
    it("appends animated placeholders without replacing existing rows or loading text buttons", async () => {
        const app = {...base(),listBusy:true}; await render(app);
        expect(host.querySelectorAll(".recent")).toHaveLength(1);
        expect(host.querySelector(".skeleton-session")).not.toBeNull();
        expect([...host.querySelectorAll(".recents button")].some(button => /load(ing)? more/i.test(button.textContent || ""))).toBe(false);
    });
    it("ignores callbacks from a disconnected server scope", async () => {
        const old = base(); await render(old);
        const observer = observed.find(entry => entry.options?.root)!;
        const next = {...base(),prefs:{...defaultPreferences,server:"http://other"}};
        await render(next);
        await intersect(observer);
        expect(old.recentMore).not.toHaveBeenCalled();
        expect(next.recentMore).not.toHaveBeenCalled();
    });
});

describe("Pinned section", () => {
    it("lists pinned chats above Recents and keeps pagination on the shared scroller", async () => {
        const app = {...base(),sessions:[
            {id:"recent",title:"Recent chat"},
            {id:"pin",title:"Pinned chat",pinned:true},
        ]};
        await render(app);
        const titles = [...host.querySelectorAll(".recent-title")].map(node => node.textContent);
        expect(host.querySelector(".session-group-heading")?.textContent).toContain("Pinned");
        expect(host.querySelector(".recents-label")?.textContent).toBe("Recents");
        expect(titles).toEqual(["Pinned chat","Recent chat"]);
        expect(host.querySelector(".recents-sentinel")).not.toBeNull();
    });
    it("hides pinned rows when the Pinned heading is collapsed", async () => {
        const app = {...base(),sessions:[{id:"pin",title:"Pinned chat",pinned:true},{id:"recent",title:"Recent chat"}]};
        await render(app);
        await act(async () => host.querySelector<HTMLButtonElement>(".session-group-heading")!.click());
        expect(host.querySelectorAll(".recent-title")).toHaveLength(1);
        expect(host.querySelector(".recent-title")?.textContent).toBe("Recent chat");
        expect(host.querySelector(".session-group-heading")?.getAttribute("aria-expanded")).toBe("false");
    });
    it("does not invent a Pinned heading when nothing is pinned", async () => {
        await render(base());
        expect(host.querySelector("#pinned-chats")).toBeNull();
        expect([...host.querySelectorAll(".session-group-heading")].some(node => node.textContent?.includes("Pinned"))).toBe(false);
    });
});
