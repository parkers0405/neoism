// @vitest-environment happy-dom
import { act, useState } from "react";
import { ChatTabs } from "./components/ChatTabs";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
const styles = readFileSync("src/style.css", "utf8");
const tabStyles = readFileSync("src/tabs.css", "utf8");

const state = vi.hoisted(() => ({ app: {} as any, composer: {} as any, footer: {} as any, library: {} as any }));
vi.mock("./components/Library", () => ({ Library: (props: any) => { state.library = props; return <div className="test-library" data-directory={props.directory} />; } }));
vi.mock("./components/ChatDetails", () => ({ ChatDetails: () => <aside className="test-chat-details" /> }));
vi.mock("./useAppController", () => ({ useAppController: () => state.app }));
vi.mock("./components/Composer", () => ({ Composer: (props: any) => {
    state.composer = props;
    return <div className="native-composer"><textarea aria-label="Message" /></div>;
}, ComposerFooter: (props: any) => { state.footer = props; return <div className="native-composer-footer">Project · tab agents / commands</div>; } }));
vi.mock("./components/Interactions", () => ({ Interactions: () => <div className="test-interactions">Question</div> }));
vi.mock("./components/Identity", () => ({ Avatar: () => <span />, Wordmark: () => <span>Wordmark</span> }));
import { App, dockingTranslation, type DockPosition } from "./App";

let root: Root, host: HTMLDivElement;
let height = 180;
const observers: { callback: () => void; disconnect: ReturnType<typeof vi.fn> }[] = [];
beforeEach(() => {
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    const storage = new Map<string, string>();
    vi.stubGlobal("localStorage", {
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, value),
    });
    height = 180;
    observers.length = 0;
    vi.stubGlobal("ResizeObserver", class {
        disconnect = vi.fn();
        constructor(public callback: () => void) { observers.push(this); }
        observe() {}
    });
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
        return { height: this.classList.contains("composer-dock") ? Math.min(height, 370) : 0, top: 0, bottom: 0, left: 0, right: 0, width: 0, x: 0, y: 0, toJSON() {} };
    });
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function(this: HTMLElement) {
        return this.classList.contains("composer-content") ? height : 1000;
    });
    state.app = {
        active: { id: "session" }, openSession: vi.fn(),
        effect: "none", nav: false, id: "session", view: "chat", tabKey: "a",
        tabs: [{ key: "a", draft: "Meaningful title", explicit: {} }],
        prefs: { name: "You", server: "http://localhost:7980", directory: "/repo" },
        directory: "/repo", recentDirectories: ["/repo", "/other"],
        sessions: [], chat: { state: { messages: [], busy: false }, loading: false },
        agent: "build", model: "provider/model", files: [], draft: "", catalog: [],
        setNav: vi.fn(), newChat: vi.fn(), activateTab: vi.fn(), closeTab: vi.fn(),
        setDirectory: vi.fn(), setSkipPermissions: vi.fn(),
    };
    host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(() => {
    act(() => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});
const render = () => act(() => root.render(<App />));

describe("application chrome and floating dock", () => {
    it.each(["skills", "workflows"])("keeps %s global when the selected project changes", kind => {
        state.app.view = kind;
        state.app.prefs.directory = "/saved-default";
        state.app.directory = "/selected-project";
        render();
        expect(state.library.directory).toBeUndefined();
        const prior = host.querySelector(".test-library");
        state.app.directory = "/another-project";
        render();
        expect(state.library.directory).toBeUndefined();
        expect(host.querySelector(".test-library")).toBe(prior);
        expect(state.app.prefs.directory).toBe("/saved-default");
    });
    it("keeps the Background word at the transcript end, not in the pinned composer", () => {
        state.app.chat.state.busy = true; // Aggregate UI busy includes this job.
        state.app.chat.activityBusy = false;
        state.app.chat.state.runtime = { rootSessionId: "session", revision: 1, branches: [],
            execution: { executionId: "e", rootSessionId: "session", rootMessageId: "u", completedMs: 0, revision: 1, finished: false, activeSegments: {} },
            runningBackgroundTasks: [{ sessionId: "session", jobId: "j", startedAt: 1 }] };
        render();
        const word = host.querySelector(".native-activity")!;
        const dock = host.querySelector(".composer-dock")!;
        expect(host.querySelectorAll(".native-activity")).toHaveLength(1);
        expect(word.parentElement).toBe(host.querySelector(".transcript"));
        expect(host.querySelector(".transcript")?.lastElementChild).toBe(word);
        expect(word.textContent).toContain("Background");
        expect(word.closest(".composer-dock, .composer-content, .composer-anchor")).toBeNull();
        expect(host.querySelector(".composer-footer-dock .native-composer-footer")).toBeNull();
        const animate = vi.spyOn(dock, "animate");
        const timeline = host.querySelector<HTMLElement>(".timeline")!;
        const dockStyle = dock.getAttribute("style");
        act(() => { timeline.scrollTop = 100; timeline.dispatchEvent(new Event("scroll", { bubbles: true })); });
        expect(host.querySelector(".native-activity")).toBe(word);
        expect(dock.getAttribute("style")).toBe(dockStyle);
        expect(animate).not.toHaveBeenCalled();
        expect(styles).toContain(".chat-main:not(.home) > .composer-dock {\n    position: absolute;");
        state.app.chat.state.runtime = { ...state.app.chat.state.runtime, runningBackgroundTasks: [] };
        render();
        expect(host.querySelector(".native-activity")).toBeNull();
    });
    it("collapses and restores desktop navigation independently of the mobile drawer", () => {
        render();
        const toggle = host.querySelector<HTMLButtonElement>(".desktop-nav-toggle")!;
        expect(toggle.getAttribute("aria-controls")).toBe("app-navigation");
        expect(toggle.getAttribute("aria-expanded")).toBe("true");
        act(() => toggle.click());
        expect(host.querySelector(".app.nav-hidden")).not.toBeNull();
        expect(toggle.getAttribute("aria-expanded")).toBe("false");
        expect(localStorage.getItem("neoism.desktop-nav-visible")).toBe("false");
        expect(state.app.setNav).not.toHaveBeenCalled();
        expect(host.querySelector(".app-chrome .desktop-nav-toggle")).not.toBeNull();
        act(() => toggle.click());
        expect(host.querySelector(".app.nav-hidden")).toBeNull();
        expect(localStorage.getItem("neoism.desktop-nav-visible")).toBe("true");
    });
    it("restores the desktop preference without hiding the mobile drawer", () => {
        localStorage.setItem("neoism.desktop-nav-visible", "false");
        state.app.nav = true;
        render();
        expect(host.querySelector(".app.nav-hidden.nav-open")).not.toBeNull();
        expect(host.querySelector(".nav-scrim")).not.toBeNull();
        expect(styles).toContain("@media (min-width: 641px)");
        expect(styles).toContain(".app.nav-hidden { grid-template-columns: 0 minmax(0, 1fr); }");
    });
    it("renders ordinary user messages as Apple-blue bubbles without author-name ownership guesses", () => {
        const selector = ".message.user:not(.runtime-message)";
        const node = document.createElement("article");
        node.className = "message user";
        expect(node.matches(selector)).toBe(true);
        node.classList.add("runtime-message");
        expect(node.matches(selector)).toBe(false);
        node.className = "message user remote-user";
        expect(node.matches(selector)).toBe(true);
        const bubble = styles.slice(styles.indexOf(selector), styles.indexOf("}", styles.indexOf(selector)));
        for (const rule of ["width: fit-content", "margin-left: auto", "max-width: min(75%, 38rem)", "padding: 12px 16px", "font-family: var(--font)", "background: #007aff", "color: #fff"]) {
            expect(bubble).toContain(rule);
        }
        expect(styles).not.toMatch(/\.message\.user\s*\{/);
    });
    it("bounds tabs while reserving close-button space and only painting keyboard focus", () => {
        expect(tabStyles).toContain("width: max-content; min-width: 120px; max-width: 220px");
        expect(tabStyles).toContain(".chat-tab-label { flex: 1; min-width: 0;");
        expect(tabStyles).toContain("text-overflow: ellipsis; white-space: nowrap");
        expect(tabStyles).toContain("flex: 0 0 27px");
        expect(tabStyles).toContain("font-family: var(--font); font-size: 13px");
        expect(tabStyles).toContain("outline: none; box-shadow: none");
        expect(tabStyles).toContain("button:focus-visible { outline: 1px solid var(--accent");
    });
    it("pins only the user identity and settings, not connection status", () => {
        state.app.connected = true;
        render();
        expect(host.querySelector(".profile")?.textContent).toBe("You");
        expect(host.querySelector(".profile small")).toBeNull();
    });

    it("keeps navigation controls and tabs in the chrome without branding", () => {
        render();
        const app = host.querySelector(".app")!;
        expect(app.querySelector(":scope > .app-chrome .brand")).toBeNull();
        expect(app.querySelector(":scope > .app-chrome img")).toBeNull();
        expect(app.querySelector(":scope > .app-chrome .desktop-nav-toggle")).not.toBeNull();
        expect(app.querySelector(":scope > .app-chrome [role=tablist]")).not.toBeNull();
        expect(app.querySelector("main .topbar")).toBeNull();
        expect(app.querySelector(".left-nav .brand")).toBeNull();
        expect(app.querySelector(".left-nav .profile")).not.toBeNull();
        const menu = app.querySelector<HTMLButtonElement>(".mobile-menu")!;
        expect(menu.getAttribute("aria-controls")).toBe("app-navigation");
        expect(menu.getAttribute("aria-expanded")).toBe("false");
        act(() => menu.click()); expect(state.app.setNav).toHaveBeenCalled();
        expect(state.composer.directory).toBe("/repo");
        expect(state.composer.recentDirectories).toEqual(["/repo", "/other"]);
        expect(state.composer.onDirectoryChange).toBe(state.app.setDirectory);
    });
    it("measures the complete dock, caps growing content, and cleans up for home", () => {
        state.app.skipPermissions = true;
        render();
        const main = host.querySelector<HTMLElement>(".chat-main")!;
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("180px");
        expect(host.querySelector(".composer-content .error-banner")).not.toBeNull();
        expect(host.querySelector(".composer-content .test-interactions")).not.toBeNull();
        height = 290;
        act(() => observers.forEach(o => o.callback()));
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("290px");
        height = 700;
        act(() => observers.forEach(o => o.callback()));
        expect(main.style.getPropertyValue("--composer-max-height")).toBe("370px");
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("370px");
        expect(host.querySelector(".composer-content.is-overflowing")).not.toBeNull();
        state.app.id = undefined;
        render();
        expect(host.querySelector(".chat-main.home .home-heading")).not.toBeNull();
        expect(host.querySelector(".composer-content.is-overflowing")).not.toBeNull();
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("380px");
        expect(observers.some(o => o.disconnect.mock.calls.length)).toBe(true);
    });
    it("tracks the visual viewport for the software keyboard", () => {
        const viewport = Object.assign(new EventTarget(), { height: 800, scale: 1 });
        vi.stubGlobal("visualViewport", viewport);
        render();
        const app = host.querySelector<HTMLElement>(".app")!;
        expect(app.style.getPropertyValue("--app-viewport-height")).toBe("800px");
        viewport.height = 400;
        act(() => viewport.dispatchEvent(new Event("resize")));
        expect(app.style.getPropertyValue("--app-viewport-height")).toBe("400px");
    });
    it("keeps external pickers outside the height-capped scroll child", () => {
        state.app.picker = "model";
        state.app.choices = [];
        state.app.choose = vi.fn();
        render();
        const panel = host.querySelector(".composer-panel")!;
        expect(panel).not.toBeNull();
        expect(panel.parentElement?.className).toBe("composer-anchor");
        expect(panel.closest(".composer-content")).toBeNull();
    });
    it("preserves tab keyboard navigation and focus restoration after closing a view", () => {
        const close = vi.fn();
        function Tabs() {
            const [tabs, setTabs] = useState(["a", "b", "c"].map(key => ({ key, draft: key, explicit: {} })));
            const [active, activate] = useState("a");
            return <ChatTabs tabs={tabs} active={active} activate={activate} add={vi.fn()} close={key => {
                close(key);
                setTabs(tabs.filter(tab => tab.key !== key));
                activate("b");
            }} />;
        }
        act(() => root.render(<Tabs />));
        const key = (value: string) => act(() => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true })));
        host.querySelector<HTMLButtonElement>('[role="tab"]')!.focus();
        key("ArrowRight"); expect(document.activeElement?.textContent).toBe("b");
        key("End"); expect(document.activeElement?.textContent).toBe("c");
        key("Home"); expect(document.activeElement?.textContent).toBe("a");
        key("Delete");
        expect(close).toHaveBeenCalledWith("a");
        expect(document.activeElement?.textContent).toBe("b");
        expect(document.activeElement?.getAttribute("aria-selected")).toBe("true");
        expect(host.querySelectorAll('[role="tab"]')).toHaveLength(2);
    });
    it("aligns messages, home heading and dock to one pane-relative responsive column", () => {
        expect(styles).toContain(".home-heading,\n.composer-dock,\n.composer-footer-dock,\n.transcript {");
        expect(styles).toContain("max-width: var(--chat-content-width)");
        expect(styles).toContain("width: min(var(--chat-content-width), calc(100cqw - 2 * var(--chat-side-gutter)))");
        expect(styles).toContain("container-type: inline-size");
        expect(styles).toContain("margin-left: max(var(--chat-side-gutter), calc((100cqw - var(--chat-content-width)) / 2))");
        expect(styles).toContain("--chat-content-width: 640px");
        expect(styles).toContain("--chat-side-gutter: clamp(32px, calc(14cqw - 16px), 128px)");
        expect(styles).toContain("--chat-side-gutter: clamp(32px, calc(14vw - 16px), 128px)");
        expect(styles).not.toContain("clamp(16px, 4cqw, 20px)");
        expect(styles).toContain("padding: 16px 0 24px");
        expect(styles).toContain("padding: 12px 0 20px");
        expect(styles).toContain("margin: 0 auto 44px");
        expect(styles).not.toContain("max-width: 1000px");
        render();
        expect(host.querySelector(".chat-main > .timeline > .transcript")).not.toBeNull();
        expect(host.querySelector(".chat-main > .composer-footer-dock .native-composer-footer")).toBeNull();
    });
    it("keeps chrome line-free and the active tab flush with the measured canvas", () => {
        expect(styles).toContain('--top-chrome: var(--surface-2)');
        expect(styles).toContain('--chat-bg: var(--bg)');
        expect(styles).toContain('--accent: #ff9da4');
        expect(styles).toContain('"chrome chrome" var(--chrome-height)');
        const topbar = styles.match(/\.topbar \{([^}]+)\}/)![1];
        expect(topbar).toContain("border: 0");
        expect(tabStyles).toContain("margin: 4px 0 0");
        expect(tabStyles).toContain("border-radius: 8px 8px 0 0");
        expect(tabStyles).toContain("background: var(--bg)");
        expect(tabStyles).not.toMatch(/border-bottom|linear-gradient|text-decoration:\s*underline/);
        expect(styles).toContain(".chat-main:not(.home) > .composer-dock {\n    position: absolute;");
        expect(styles).toContain("padding-bottom: calc(var(--composer-clearance, 180px)");
        expect(styles).toContain("--chat-content-width: 640px");
        expect(styles).toContain("--chat-side-gutter: clamp(32px, calc(14vw - 16px), 128px)");
    });
});


describe("subagent read-only layout", () => {
    const noInput = () => {
        expect(host.querySelector("textarea, .composer-anchor, .native-composer-footer, .composer-panel")).toBeNull();
        expect(host.querySelector(".chat-main")?.classList.contains("without-composer")).toBe(true);
        expect(host.querySelector(".composer-dock .composer-content")).toBeNull();
    };
    it("withholds stale history and input until matching root metadata hydrates, then measures refs", () => {
        state.app.active = { id: "previous", parentId: "old" };
        state.app.chat.state.messages = [{ info: { id: "stale", role: "user" }, parts: [{ id: "text", type: "text", text: "Stale message" }] }];
        render(); noInput();
        expect(host.textContent).toContain("Loading conversation");
        expect(host.textContent).not.toContain("Stale message");
        expect(host.querySelector(".subagent-view-hint")).toBeNull();
        state.app.active = { id: "session" }; render();
        expect(host.querySelector("textarea")).not.toBeNull();
        expect(host.querySelector<HTMLElement>(".chat-main")!.style.getPropertyValue("--composer-clearance")).toBe("180px");
    });
    it("removes the whole dock on child navigation and Back only opens the cached root", () => {
        render();
        state.app.active = { id: "session", parentId: "parent" };
        state.app.chat.state.runtime = { rootSessionId: "root", branches: [], revision: 1 };
        state.app.picker = "directory";
        render(); noInput();
        expect(host.querySelector(".folder-picker")).toBeNull();
        const hint = host.querySelector(".subagent-view-hint")!;
        expect(hint.querySelector("button")?.textContent).toBe("Back to main chat");
        expect(hint.textContent).toContain("Subagent");
        act(() => hint.querySelector<HTMLButtonElement>("button")!.click());
        expect(state.app.openSession).toHaveBeenCalledExactlyOnceWith("root");
        expect(state.app.closeTab).not.toHaveBeenCalled();
        const css = readFileSync("src/subagent-view.css", "utf8");
        expect(css).toContain("padding-bottom: calc(var(--composer-clearance, 0px) + max(24px, env(safe-area-inset-bottom)))");
    });
    it("keeps child history and live activity readable without a send surface", () => {
        state.app.active = { id: "session", parentId: "parent" };
        state.app.chat.state.busy = true;
        state.app.chat.activityBusy = true;
        state.app.chat.state.messages = [{ info: { id: "child-message", sessionId: "session", role: "assistant" }, parts: [{ id: "text", type: "text", text: "Child progress" }] }];
        render(); noInput();
        expect(host.querySelector(".transcript")?.textContent).toContain("Child progress");
        expect(host.querySelector(".transcript .native-activity")?.textContent).toContain("Crafting");
        expect(host.querySelector(".composer-dock")).toBeNull();
    });
    it("hydrates a nested child without ever mounting an input and labels parent fallback honestly", () => {
        state.app.active = undefined; render(); noInput();
        state.app.active = { id: "session", parentId: "nested-parent" }; render(); noInput();
        const back = host.querySelector<HTMLButtonElement>(".subagent-view-hint button")!;
        expect(back.textContent).toBe("Back to parent chat");
        act(() => back.click());
        expect(state.app.openSession).toHaveBeenCalledWith("nested-parent");
        state.app.active = { id: "session", agent: "explore" }; render();
        expect(host.querySelector("textarea")).not.toBeNull();
    });
});

describe("composer docking contract", () => {
    it.each(["send", "reduced", "other-tab", "resize", "unknown", "child", "navigation"])("first-send sidebar narrowing: %s", async mode => {
        state.app.id = undefined;
        state.app.send = vi.fn().mockResolvedValue(undefined);
        const motion = Object.assign(new EventTarget(), { matches: mode === "reduced" });
        vi.stubGlobal("matchMedia", () => motion);
        const animate = vi.fn(() => ({ cancel: vi.fn() }));
        vi.stubGlobal("innerWidth", 1200);
        const original = HTMLElement.prototype.animate;
        HTMLElement.prototype.animate = animate as any;
        vi.mocked(HTMLElement.prototype.getBoundingClientRect).mockImplementation(function(this: HTMLElement) {
            const home = !!this.closest(".home");
            return { top: home ? 200 : 700, left: home ? 80 : 30, width: home ? 800 : 540,
                height: home ? 240 : 140, bottom: 0, right: 0, x: 0, y: 0, toJSON() {} };
        });
        try {
            render();
            const input = host.querySelector<HTMLTextAreaElement>("textarea")!;
            input.focus();
            if (mode !== "navigation") await act(async () => { await state.composer.send("First message"); });
            state.app.id = "sent";
            state.app.active = mode === "unknown" ? undefined : { id: "sent", ...(mode === "child" ? { parentId: "root" } : {}) };
            state.app.sidebar = true;
            if (mode === "other-tab") state.app.tabKey = "b";
            if (mode === "resize") vi.stubGlobal("innerWidth", 900);
            render();
            if (mode === "send") {
                expect(animate).toHaveBeenCalledExactlyOnceWith([
                    { transform: "translate(50px, -500px)" }, { transform: "translate(0, 0)" },
                ], { duration: 260, easing: "cubic-bezier(.2,.8,.2,1)" });
                expect(host.querySelector("textarea")).toBe(input);
                expect(document.activeElement).toBe(input);
                expect(host.querySelector(".test-chat-details")).not.toBeNull();
            } else expect(animate).not.toHaveBeenCalled();
        } finally { HTMLElement.prototype.animate = original; }
    });
    it("uses the freed chat footer space while attachments and input grow", () => {
        render();
        const main = host.querySelector<HTMLElement>(".chat-main")!;
        expect(main.style.getPropertyValue("--composer-footer-height")).toBe("0px");
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("180px");
        height = 300;
        act(() => observers.forEach(o => o.callback()));
        expect(main.style.getPropertyValue("--composer-footer-height")).toBe("0px");
        expect(main.style.getPropertyValue("--composer-clearance")).toBe("300px");
        expect(main.style.getPropertyValue("--composer-max-height")).toBe("370px");
        expect(host.querySelector(".composer-footer-dock")).toBeNull();
    });
    it("keeps home hints but passes chat busy state into the composer without a footer", () => {
        state.app.id = undefined; state.app.chat.state.busy = true; render();
        expect(state.footer.showProject).toBe(true);
        expect(host.querySelector(".composer-footer-dock")).not.toBeNull();
        state.app.id = "session"; render();
        expect(host.querySelector(".composer-footer-dock")).toBeNull();
        expect(state.composer.busy).toBe(true);
        state.app.chat.state.busy = false; render();
        expect(state.composer.busy).toBe(false);
    });
    const home: DockPosition = { top: 200, left: 20, width: 800, viewportHeight: 900, viewportWidth: 1200, home: true, tabKey: "a" };
    const chat = { ...home, top: 700, home: false };
    it("uses measured translation only for the same home tab becoming a conversation", () => {
        expect(dockingTranslation(home, chat, false)).toEqual({ x: 0, y: -500 });
        expect(dockingTranslation(home, chat, true)).toBeUndefined();
        expect(dockingTranslation(undefined, chat, false)).toBeUndefined();
        expect(dockingTranslation(home, { ...chat, tabKey: "b" }, false)).toBeUndefined();
        expect(dockingTranslation(chat, chat, false)).toBeUndefined();
        expect(dockingTranslation(home, { ...chat, top: home.top }, false)).toBeUndefined();
    });
    it("rejects resized viewports and invalid geometry instead of flying under the pointer", () => {
        expect(dockingTranslation(home, { ...chat, viewportHeight: 400 }, false)).toBeUndefined();
        expect(dockingTranslation(home, { ...chat, viewportWidth: 400 }, false)).toBeUndefined();
        expect(dockingTranslation(home, { ...chat, width: 400 }, false)).toEqual({ x: 0, y: -500 });
        expect(dockingTranslation(home, { ...chat, top: NaN }, false)).toBeUndefined();
    });
    it("keeps one input and removes the home footer after first send", () => {
        state.app.id = undefined; render();
        const input = host.querySelector("textarea");
        const footer = host.querySelector(".native-composer-footer")!;
        expect(footer.parentElement?.className).toBe("composer-footer-dock");
        expect(footer.closest(".composer-content, .composer-anchor, .timeline")).toBeNull();
        expect(footer.parentElement?.parentElement?.classList.contains("chat-main")).toBe(true);
        expect(styles).toContain("bottom: calc(var(--composer-gutter) + var(--composer-footer-height, 0px))");
        expect(styles).toContain(".composer-footer-dock {\n    position: relative;\n    flex-shrink: 0;\n    margin: 10px auto 0;");
        expect(footer.parentElement?.previousElementSibling).toBe(host.querySelector(".composer-dock"));
        expect(state.composer.showFooter).toBe(false);
        state.app.id = "sent"; state.app.active = { id: "sent" }; render();
        expect(host.querySelector("textarea")).toBe(input);
        expect(host.querySelector(".native-composer-footer")).toBeNull();
    });
});
