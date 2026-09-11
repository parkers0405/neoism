import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import type { Session } from "@neoism/sdk";

// Deterministic hook host: exercise controller requests/effects without a browser
// or a new DOM dependency. State setters and memo/effect dependencies follow React.
const host = vi.hoisted(() => {
    let slots: any[] = [], cursor = 0;
    let effects: (() => void)[] = [];
    let dirty = false;
    const equal = (a?: unknown[], b?: unknown[]) => !!a && !!b && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
    return {
        clients: new Map<string, any>(),
        transports: [] as { baseUrl: string; token?: string }[],
        chat: { state: { messages: [], busy: false }, setState: vi.fn(), markCreatedSession: vi.fn(), beginPrompt: vi.fn(() => 'msg_local_prompt') },
        reset() { slots = []; cursor = 0; effects = []; dirty = false; this.transports.length = 0; },
        begin() { cursor = 0; dirty = false; },
        flush() { const pending = effects; effects = []; pending.forEach((f) => f()); return dirty; },
        cleanup() { slots.forEach((s) => s?.cleanup?.()); },
        useState(initial: any) {
            const i = cursor++;
            if (!slots[i]) slots[i] = { value: typeof initial === "function" ? initial() : initial,
                set: (next: any) => { const value = typeof next === "function" ? next(slots[i].value) : next; if (!Object.is(value, slots[i].value)) { slots[i].value = value; dirty = true; } } };
            return [slots[i].value, slots[i].set];
        },
        useRef(initial: any) { const i = cursor++; return slots[i] ||= { current: initial }; },
        useMemo(fn: () => any, deps: any[]) { const i = cursor++; if (!slots[i] || !equal(slots[i].deps, deps)) slots[i] = { value: fn(), deps }; return slots[i].value; },
        useEffect(fn: () => any, deps: any[]) {
            const i = cursor++;
            if (slots[i] && equal(slots[i].deps, deps)) return;
            const old = slots[i]; slots[i] = { deps };
            effects.push(() => { old?.cleanup?.(); slots[i].cleanup = fn(); });
        },
    };
});
vi.mock("react", () => ({ ...host, useCallback: (fn: any, deps: any[]) => host.useMemo(() => fn, deps) }));
vi.mock("@neoism/sdk", () => ({ createHttpTransport: (input: any) => { host.transports.push({ ...input }); return input; }, createNeoismClient: (input: any) => host.clients.get(input.baseUrl) }));
vi.mock("./useChat", () => ({ useChat: () => host.chat }));
vi.mock("./nativeCommands", () => ({ nativeCommand: vi.fn(async (_name, _args, context) => { if (_name === "goal") await context.sendPrompt(context.id, _args); return true; }) }));
import { SessionPinIndex } from "./sessionPins";
import { useAppController } from "./useAppController";
import { recentSessions, mergeSession } from "./useSessionEvents";
import { rememberedSession, rememberSession, serverScope, loadDeletedAccounts, loadPreferences } from "./types";

const session = (id: string, extra: Partial<Session> = {}): Session => ({ id, title: id, directory: "/work", projectId: "p", slug: id, version: "1", time: { created: 1, updated: 1 }, ...extra });
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (e: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function client() {
    const events: any[] = [];
    let wake: (() => void) | undefined;
    return {
        emit(event: any) { events.push(event); wake?.(); },
        events: { async *subscribe({ signal }: { signal: AbortSignal }) {
            while (!signal.aborted) {
                if (!events.length) await new Promise<void>((resolve) => { wake = resolve; signal.addEventListener("abort", resolve as () => void, { once: true }); });
                if (signal.aborted) break;
                while (events.length) yield events.shift();
            }
        } },
        config: { defaults: vi.fn(async (_directory?: string) => ({ defaultAgent: null as string | null, model: null as string | null, variant: null as string | null })) },
        artifacts: { upload: vi.fn(async (file: File) => ({ filename: file.name, mediaType: file.type, downloadUrl: "https://server/artifact/file" })) },
        health: { get: vi.fn(async () => ({})) },
        catalog: { commands: { list: vi.fn(async () => []) }, providers: { configured: vi.fn(async () => ({ providers: providers.all, default: providers.default })), list: vi.fn(async () => ({ all: [], connected: [] })) }, agents: { list: vi.fn(async () => [] as {name: string; mode?: string; hidden?: boolean; description?: string}[]) }, skills: { list: vi.fn(async () => [{ name: "review", description: "Review code" }]) } },
        sessions: {
            list: vi.fn(async () => ({ items: [] as Session[], cursor: { next: undefined as string | undefined } })),
            get: vi.fn(async (id: string) => session(id)), create: vi.fn(async () => session("created")),
            update: vi.fn(async (id: string, patch: any) => session(id, patch)),
            pin: vi.fn(async (id: string, pinned: boolean) => session(id, { pinned })),
            delete: vi.fn(async () => {}), abort: vi.fn(async () => {}),
            prompt: vi.fn(async () => {}), command: vi.fn(async () => ({})),
        },
        operations: { request: vi.fn(async (name: string) => name.endsWith("directoryOptions") ? ["/work", "/other"] : { items: [session("child", { parentId: "a" })] }) },
        interactions: { permissions: { list: vi.fn(async () => []) }, questions: { list: vi.fn(async () => []) } },
    };
}
let app: ReturnType<typeof useAppController>;
let server: ReturnType<typeof client>;
function render() { let again = true, count = 0; while (again) { if (++count > 20) throw new Error("Render loop"); host.begin(); app = useAppController(); again = host.flush(); } return app; }
async function settle() { for (let i = 0; i < 40; i++) { await Promise.resolve(); render(); } }
beforeEach(() => {
    vi.useFakeTimers(); host.reset(); host.chat.setState.mockClear(); host.chat.markCreatedSession.mockClear(); host.chat.beginPrompt.mockClear();
    const storage = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key) });
    const location = new URL("http://localhost:5174/");
    vi.stubGlobal("window", { innerWidth: 1200, confirm: () => true, location, addEventListener: vi.fn(), removeEventListener: vi.fn(), history: { pushState: (_a: any, _b: any, url: URL) => { location.href = url.href; }, replaceState: (_a: any, _b: any, url: URL) => { location.href = url.href; } } });
    vi.stubGlobal("document", { documentElement: { style: { setProperty() {}, removeProperty() {} } } });
    server = client(); host.clients.set("http://127.0.0.1:4096", server); render();
});
afterEach(() => { host.cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("native session pins", () => {
    it("handles native unpin responses with the metadata key removed", async () => {
        await app.pinSession('a',true); render();
        server.sessions.pin.mockResolvedValue(session('a'));
        await app.pinSession('a',false); render();
        expect(app.sessions[0].pinned).toBeUndefined();
        expect(new SessionPinIndex(app.prefs.server).ids.size).toBe(0);
        expect(mergeSession(session('a',{pinned:true}),session('a')).pinned).toBeUndefined();
    });
    it("discards old authenticated hydration and callbacks when the token changes", async () => {
        new SessionPinIndex(app.prefs.server).observe(session('secret',{pinned:true}));
        const pending = deferred<Session>(); server.sessions.get.mockReturnValue(pending.promise);
        host.cleanup(); host.reset(); render();
        const old = app, authenticated = client();
        host.clients.set(app.prefs.server,authenticated);
        app.setToken('new-token'); render();
        pending.resolve(session('secret',{pinned:true})); await settle();
        await old.pinSession('secret',true); await old.renameSession('secret','changed'); await old.deleteSession('secret');
        expect(server.sessions.pin).not.toHaveBeenCalled(); expect(server.sessions.update).not.toHaveBeenCalled(); expect(server.sessions.delete).not.toHaveBeenCalled();
        expect(app.sessions).toEqual([]);
        expect(authenticated.sessions.get).toHaveBeenCalledWith('secret');
        expect(host.transports.at(-1)?.token).toBe('new-token');
    });
    it("rejects old pagination callbacks after the search changes", async () => {
        server.sessions.list.mockResolvedValue({items:[session('a')],cursor:{next:'page2'}});
        await vi.advanceTimersByTimeAsync(180); await settle();
        const old = app;
        app.setSearch('new query'); render();
        const count = server.sessions.list.mock.calls.length;
        await old.recentMore(); expect(server.sessions.list).toHaveBeenCalledTimes(count);
    });
    it("hydrates old pinned IDs after reload, filters roots/directory/search and deduplicates pages", async () => {
        const index = new SessionPinIndex(app.prefs.server);
        for (const id of ['old', 'other', 'child']) index.observe(session(id, { pinned: true }));
        host.cleanup(); host.reset();
        server.sessions.get.mockImplementation(async id => session(id, { pinned: true, title: 'match', directory: id === 'other' ? '/elsewhere' : '/work', parentId: id === 'child' ? 'parent' : undefined }));
        server.sessions.list.mockResolvedValue({ items: [session('new', { title: 'match', time: {created:2,updated:100} })], cursor: { next: 'page2' } });
        render(); app.setPrefs({...app.prefs, directory:'/work'}); app.setSearch('match'); render();
        await vi.advanceTimersByTimeAsync(180); await settle();
        expect(app.sessions.map(s => s.id)).toEqual(['old','new']);
        server.sessions.list.mockResolvedValue({items:[session('old', {pinned:true,title:'match'}),session('older',{title:'match'})],cursor:{next:undefined}});
        await app.recentMore(); render();
        expect(app.sessions.map(s => s.id)).toEqual(['old','new','older']);
        expect(server.sessions.list.mock.calls.every(([options]: any[]) => options.roots === true && options.limit === 30)).toBe(true);
    });
    it("persists pins through the API, rename preserves pin metadata and delete removes the index", async () => {
        await app.pinSession('a', true); render();
        expect(server.sessions.pin).toHaveBeenCalledWith('a',true);
        expect(new SessionPinIndex(app.prefs.server).ids.has('a')).toBe(true);
        server.sessions.update.mockResolvedValue(session('a',{title:'Renamed',pinned:true}));
        await app.renameSession('a','Renamed'); render();
        expect(app.sessions[0]).toMatchObject({title:'Renamed',pinned:true});
        await app.deleteSession('a'); render();
        expect(app.sessions).toEqual([]); expect(new SessionPinIndex(app.prefs.server).ids.size).toBe(0);
    });
    it("does not optimistically pin on failure and tolerates unavailable browser persistence", async () => {
        server.sessions.pin.mockRejectedValueOnce(new Error('pin failed'));
        await app.pinSession('a',true); render();
        expect(app.sessions).toEqual([]); expect(app.error).toBe('pin failed');
        vi.spyOn(localStorage,'setItem').mockImplementation(() => {throw new Error('quota');});
        await app.pinSession('a',true); render();
        expect(app.sessions[0].pinned).toBe(true);
    });
    it("does not resurrect a deleted session from an outstanding pin or rename", async () => {
        const pending = deferred<Session>(); server.sessions.pin.mockReturnValue(pending.promise);
        const renaming = deferred<Session>(); server.sessions.update.mockReturnValue(renaming.promise);
        const pin = app.pinSession('a',true), rename = app.renameSession('a','later');
        await app.deleteSession('a'); app.setSearch('later'); render();
        pending.resolve(session('a',{pinned:true,title:'later'})); renaming.resolve(session('a',{title:'later'}));
        await pin; await rename; render();
        expect(app.sessions).toEqual([]); expect(new SessionPinIndex(app.prefs.server).ids.size).toBe(0);
    });
    it("rejects old callbacks and pending mutations after a server swap", async () => {
        const old = app; const pending = deferred<Session>(); server.sessions.pin.mockReturnValue(pending.promise);
        const request = app.pinSession('secret',true);
        const other = client(); host.clients.set('http://other:4096',other);
        app.setPrefs({...app.prefs,server:'http://other:4096'}); render();
        pending.resolve(session('secret',{pinned:true})); await request;
        await old.pinSession('old',true); await old.renameSession('old','old'); await old.deleteSession('old'); render();
        expect(server.sessions.pin).toHaveBeenCalledTimes(1);
        expect(server.sessions.update).not.toHaveBeenCalled(); expect(server.sessions.delete).not.toHaveBeenCalled();
        expect(app.sessions).toEqual([]); expect(new SessionPinIndex(app.prefs.server).ids.size).toBe(0);
    });
    it("drops old hydration results on server changes", async () => {
        new SessionPinIndex(app.prefs.server).observe(session('secret',{pinned:true}));
        const pending = deferred<Session>(); server.sessions.get.mockReturnValue(pending.promise);
        host.cleanup(); host.reset(); render();
        expect(server.sessions.get).toHaveBeenCalledWith('secret');
        const other = client(); host.clients.set('http://other:4096',other);
        app.setPrefs({...app.prefs,server:'http://other:4096'}); render();
        pending.resolve(session('secret',{pinned:true})); await settle();
        expect(app.sessions).toEqual([]);
    });
});

describe("click latency request and state contracts", () => {
    it('toggles chips but repeated slash commands explicitly open the picker', async () => {
        await settle();
        app.togglePicker('model'); render(); expect(app.picker).toBe('model');
        app.togglePicker('model'); render(); expect(app.picker).toBeUndefined();
        app.togglePicker('agent'); render(); app.togglePicker('thinking'); render(); expect(app.picker).toBe('thinking');
        await app.send('/model'); render(); expect(app.picker).toBe('model');
        await app.send('/model'); render(); expect(app.picker).toBe('model');
    });
    it('reports only real catalog pending work and clears loading on success or failure', async () => {
        await settle();
        const pending = deferred<any>(); server.catalog.providers.configured.mockReturnValue(pending.promise);
        app.setPrefs(p => ({ ...p, directory: '/pending-models' })); app.setPicker('model'); await settle();
        expect(app.pickerLoading).toBe(true);
        pending.resolve({ providers: [], default: {} }); await settle(); expect(app.pickerLoading).toBe(false);
        const agents = deferred<any>(); server.catalog.agents.list.mockReturnValue(agents.promise);
        app.setPrefs(p => ({ ...p, directory: '/pending-agents' })); app.setPicker('agent'); await settle();
        expect(app.pickerLoading).toBe(true); expect(app.choices).toEqual([]);
        agents.reject(new Error('agents unavailable')); await settle(); expect(app.pickerLoading).toBe(false);
    });
    it('hydrates configured models without polluting recent user selections', async () => {
        await settle();
        expect(localStorage.getItem('neoism.gui.recent-models:' + serverScope(app.prefs.server))).toBeNull();
        app.setModel('openai/chosen'); render(); app.setPicker('model'); render();
        expect(app.choices.filter(c => c.section === 'Recent').map(c => c.id)).toEqual(['openai/chosen']);
        expect(app.choices[0].badge).toBe('Selected · unavailable');
        expect(server.catalog.providers.configured).toHaveBeenCalled();
    });
    it("does not fetch catalogs or persist tabs on model/agent/thinking opens", async () => {
        await settle(); vi.advanceTimersByTime(0);
        const writes = vi.spyOn(localStorage, 'setItem');
        const requests = [server.health.get, server.catalog.commands.list, server.catalog.agents.list, server.catalog.providers.list, server.catalog.providers.configured, server.config.defaults];
        const before = requests.map(r => r.mock.calls.length);
        const usage = app.usage, openSession = app.openSession;
        for (const picker of ['model', 'agent', 'thinking']) {
            app.setPicker(picker); await settle();
            expect(app.picker).toBe(picker);
            expect(app.usage).toBe(usage); expect(app.openSession).toBe(openSession);
        }
        expect(requests.map(r => r.mock.calls.length)).toEqual(before);
        expect(writes.mock.calls.filter(([key]) => key.startsWith('neoism.gui.tabs:'))).toHaveLength(0);
    });
    it("selects a tab before metadata resolves without refetching same-directory catalogs", async () => {
        await settle();
        const first = app.tabKey;
        app.newChat(); await settle();
        const second = app.tabKey;
        const requests = [server.health.get, server.catalog.commands.list, server.catalog.agents.list, server.catalog.providers.list, server.catalog.providers.configured, server.config.defaults];
        const before = requests.map(r => r.mock.calls.length);
        for (let i = 0; i < 10; i++) { app.activateTab(i % 2 ? second : first); await settle(); }
        expect(requests.map(r => r.mock.calls.length)).toEqual(before);
        const pending = deferred<Session>(); server.sessions.get.mockReturnValue(pending.promise);
        app.openSession('pending'); render();
        expect(app.id).toBe('pending');
        expect(app.tabs.find(t => t.key === app.tabKey)?.sessionId).toBe('pending');
        expect(server.sessions.get).toHaveBeenCalledWith('pending');
        pending.resolve(session('pending')); await settle();
    });
    it("coalesces draft persistence while keeping draft state immediate", async () => {
        await settle(); vi.advanceTimersByTime(0);
        const writes = vi.spyOn(localStorage, 'setItem');
        for (let i = 0; i < 100; i++) app.onDraftChange(String(i));
        render(); expect(app.draft).toBe('99');
        expect(writes.mock.calls.filter(([key]) => key.startsWith('neoism.gui.tabs:'))).toHaveLength(0);
        vi.advanceTimersByTime(0);
        expect(writes.mock.calls.filter(([key]) => key.startsWith('neoism.gui.tabs:'))).toHaveLength(1);
    });
});

describe("controller request ownership", () => {
    it("single-flights creation and preserves model/agent/variant/account/directory", async () => {
        app.setPrefs((p) => ({ ...p, directory: "/chosen" })); app.setModel("openai/gpt-5.6"); app.setAgent("plan"); app.setThinking("ultra"); app.setConnectionId("account"); render();
        const pending = deferred<Session>(); server.sessions.create.mockReturnValue(pending.promise);
        const first = app.send("one"), second = app.send("two");
        await settle();
        expect(server.sessions.create).toHaveBeenCalledTimes(1);
        expect(server.sessions.create).toHaveBeenCalledWith({ directory: "/chosen", agent: "plan", model: { providerId: "openai", id: "gpt-5.6", variant: "ultra", connectionId: "account" } });
        pending.resolve(session("created")); await Promise.all([first, second]); render();
        expect(app.id).toBe("created"); expect(server.sessions.prompt).toHaveBeenCalledTimes(2);
        expect(host.chat.markCreatedSession).toHaveBeenCalledWith('created');
        expect(host.chat.beginPrompt).toHaveBeenCalledWith('created');
        expect(server.sessions.prompt).toHaveBeenCalledWith('created', expect.objectContaining({ messageId: 'msg_local_prompt' }));
        expect(server.sessions.prompt.mock.calls[0]).toEqual(["created", expect.objectContaining({ agent: "plan", variant: "ultra", model: { providerId: "openai", modelId: "gpt-5.6", variant: "ultra", connectionId: "account" } })]);
    });
    it("binds and submits creation to the originating tab without selecting it after switching", async () => {
        const pending = deferred<Session>(); server.sessions.create.mockReturnValue(pending.promise);
        const original = app.tabKey; const sending = app.send("old");
        app.openSession("new"); pending.resolve(session("old")); await sending; await settle();
        expect(app.id).toBe("new"); expect(server.sessions.prompt).toHaveBeenCalledWith("old", expect.objectContaining({ prompt: "old" })); expect(app.tabs.find(t => t.key === original)?.sessionId).toBe("old"); expect(app.error).toBe("");
    });
    it("ignores out-of-order metadata and retains active session through recents search", async () => {
        const old = deferred<Session>(); server.sessions.get.mockImplementation((id) => id === "old" ? old.promise : Promise.resolve(session(id, { model: { providerId: "p", id: "m", variant: "high", connectionId: "c" }, agent: "plan" })));
        app.openSession("old"); app.openSession("new"); await settle();
        old.resolve(session("old", { agent: "wrong" })); await settle();
        app.setSearch("no match"); render(); await vi.advanceTimersByTimeAsync(180); await settle();
        expect(app.active?.id).toBe("new"); expect(app.agent).toBe("plan"); expect(app.thinking).toBe("high"); expect(app.connectionId).toBe("c"); expect(app.sessions).toEqual([]);
    });
    it("invalidates old server requests and suppresses their errors", async () => {
        const pending = deferred<Session>(); server.sessions.get.mockReturnValue(pending.promise); app.openSession("old");
        const other = client(); host.clients.set("http://other", other); app.setPrefs((p) => ({ ...p, server: "http://other" })); render();
        pending.reject(new Error("old server failure")); await settle();
        expect(app.id).toBeUndefined(); expect(app.active).toBeUndefined(); expect(app.error).toBe("");
    });
    it("does not mark a different chat busy when an earlier prompt completes", async () => {
        app.openSession("a"); await settle(); const pending = deferred<void>(); server.sessions.prompt.mockReturnValue(pending.promise);
        const sending = app.send("hello"); await settle(); app.openSession("b"); pending.resolve(); await sending; await settle();
        expect(host.chat.setState).not.toHaveBeenCalled();
    });
});

describe("controller parity", () => {
    it("persists a closed right panel and keeps it closed when opening another chat", async () => {
        expect(app.sidebar).toBe(false);
        app.setSidebar(false); await settle();
        expect(app.sidebar).toBe(false);
        expect(localStorage.getItem("neoism.gui.details-visible")).toBe("false");
        app.newChat(); await settle();
        expect(app.sidebar).toBe(false);
        app.setSidebar(value => !value); await settle();
        expect(localStorage.getItem("neoism.gui.details-visible")).toBe("true");
    });
    it("migrates only the rejected gray default and keeps explicit native themes", () => {
        localStorage.setItem("neoism.gui.preferences", JSON.stringify({theme:"neoism"}));
        expect(loadPreferences().theme).toBe("pastelbeans");
        localStorage.setItem("neoism.gui.preferences", JSON.stringify({theme:"tokyo_night"}));
        expect(loadPreferences().theme).toBe("tokyo_night");
    });
    it("matches native primary-agent filtering in the picker and Tab cycling", async () => {
        server.catalog.agents.list.mockResolvedValue([
            {name:"build", mode:"primary"}, {name:"plan", mode:"primary"},
            {name:"summary", hidden:true}, {name:"explore", mode:"subagent"},
            {name:"general", mode:"subagent"}, {name:"my-agent", mode:"all"},
        ]);
        app.setPrefs(p => ({...p, directory:"/agents-test"})); await settle();
        app.setPicker("agent"); render();
        expect(app.choices.map(choice => choice.id)).toEqual(["build", "plan", "my-agent"]);
        app.setAgent("plan"); await settle(); app.onCycleAgent(); await settle();
        expect(app.agent).toBe("my-agent");
        await expect(app.send("/agent explore")).rejects.toThrow("Unknown agent");
    });
    it("changes session directory without changing identity or global directory", async () => {
        app.openSession("a"); await settle(); await app.send("/cd /other"); await settle();
        expect(server.sessions.update).toHaveBeenCalledWith("a", { directory: "/other" }); expect(app.id).toBe("a"); expect(app.prefs.directory).toBe(""); expect(app.active?.directory).toBe("/other");
        await app.send("/cd"); await settle(); expect(app.picker).toBe("directory"); expect(app.choices.map((c) => c.id)).toContain("/other");
    });
    it("inserts skill mentions without sending and loads a chooser for bare skill", async () => {
        await app.send("/skill review"); render(); expect(app.draftInsertion?.text).toBe("$review "); expect(server.sessions.prompt).not.toHaveBeenCalled(); expect(server.sessions.create).not.toHaveBeenCalled();
        await app.send("/skill"); await settle(); expect(app.picker).toBe("skill"); app.choose("review"); render(); expect(app.draftInsertion?.revision).toBe(2);
    });
    it("patches active choices and offers ultra for gpt-5.6", async () => {
        app.openSession("a"); await settle(); app.setModel("openai/gpt-5.6"); app.setThinking("ultra"); app.setConnectionId("acct"); app.setAgent("plan"); await settle();
        expect(server.sessions.update).toHaveBeenCalledWith("a", { model: { providerId: "openai", id: "gpt-5.6", variant: "ultra", connectionId: "acct" } });
        expect(server.sessions.update).toHaveBeenCalledWith("a", { agent: "plan" }); app.setPicker("thinking"); render(); expect(app.choices.some((c) => c.id === "ultra")).toBe(true);
    });
    it("requires an active session for interactions", async () => {
        await expect(app.send("/permissions")).rejects.toThrow("Open a chat"); await expect(app.send("/questions")).rejects.toThrow("Open a chat");
        expect(server.interactions.permissions.list).not.toHaveBeenCalled(); expect(server.interactions.questions.list).not.toHaveBeenCalled();
    });
    it("goal starts a session and supplies the scoped initial-prompt callback", async () => {
        await app.send("/goal ship it"); expect(server.sessions.create).toHaveBeenCalledTimes(1); expect(server.sessions.prompt).toHaveBeenCalledWith("created", expect.objectContaining({ prompt: "ship it" }));
    });
    it("loads child sessions separately from root Recents", async () => {
        app.openSession("a"); await settle(); app.setPicker("subagents"); await settle(); expect(app.choices.map((c) => c.id)).toEqual(["child"]); expect(app.sessions).toEqual([]);
    });
});

describe("account and FX integration", () => {
    it("persists per-provider accounts and blocks a deleted account without fallback", async () => {
        app.setModel("openai/gpt-5.6"); render();
        app.onSelectConnection({ providerId: "openai", connectionId: "acct", label: "Work", reason: "selected" }); render();
        app.setModel("other/model"); app.setModel("openai/gpt-5.6"); render(); expect(app.connectionId).toBe("acct");
        app.onSelectConnection({ providerId: "openai", connectionId: "acct", label: "Work", reason: "deleted" }); render();
        await expect(app.send("hello")).rejects.toThrow("account was deleted"); expect(server.sessions.prompt).not.toHaveBeenCalled();
        app.onSelectConnection({ providerId: "openai", connectionId: "replacement", label: "Personal", reason: "selected" }); render();
        await app.send("hello"); expect(server.sessions.prompt).toHaveBeenCalledWith("created", expect.objectContaining({ model: expect.objectContaining({ connectionId: "replacement" }) }));
    });
    it("forwards server commands with account and reasoning selections", async () => {
        app.setModel("openai/gpt-5.6"); app.setConnectionId("acct"); app.setThinking("ultra"); render();
        // The native mock handles known operations; make this unknown command fall through.
        const { nativeCommand } = await import("./nativeCommands"); vi.mocked(nativeCommand).mockResolvedValueOnce(false);
        await app.send("/server-only hello");
        expect(server.sessions.command).toHaveBeenCalledWith("created", "server-only", expect.objectContaining({ arguments: "hello", model: { providerId: "openai", modelId: "gpt-5.6", variant: "ultra", connectionId: "acct" } }));
    });
    it("restarts repeated FX and dispatches exactly one timed prompt", async () => {
        app.openSession("a"); await settle(); await app.send("/disco"); render();
        await vi.advanceTimersByTimeAsync(1000); await app.send("/disco"); render();
        await vi.advanceTimersByTimeAsync(1000); expect(server.sessions.prompt).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(1000); await settle(); expect(server.sessions.prompt).toHaveBeenCalledTimes(1);
        await vi.advanceTimersByTimeAsync(6000); await settle(); expect(app.effect).toBe("");
    });
    it("cancels FX on session and directory changes", async () => {
        app.openSession("a"); await settle(); await app.send("/praise"); render(); app.newChat(); render();
        await vi.advanceTimersByTimeAsync(10000); expect(server.sessions.prompt).not.toHaveBeenCalled();
        await app.send("/disco"); render(); app.setPrefs((p) => ({ ...p, directory: "/else" })); render();
        await vi.advanceTimersByTimeAsync(10000); expect(server.sessions.prompt).not.toHaveBeenCalled(); expect(app.effect).toBe("");
    });
    it("does not dispatch an FX prompt after asynchronous creation changes scope", async () => {
        const pending = deferred<Session>(); server.sessions.create.mockReturnValue(pending.promise);
        await app.send("/disco"); render(); await vi.advanceTimersByTimeAsync(2000);
        app.openSession("a"); render(); pending.resolve(session("created")); await settle(); expect(server.sessions.prompt).not.toHaveBeenCalled(); expect(app.id).toBe("a");
    });
    it("waits for session metadata before sending its restored selections", async () => {
        const pending = deferred<Session>(); server.sessions.get.mockReturnValue(pending.promise);
        app.openSession("a"); render(); const sending = app.send("hello"); await settle(); expect(server.sessions.prompt).not.toHaveBeenCalled();
        pending.resolve(session("a", { model: { providerId: "p", id: "m", connectionId: "c", variant: "high" }, agent: "plan" })); await sending;
        expect(server.sessions.prompt).toHaveBeenCalledWith("a", expect.objectContaining({ agent: "plan", variant: "high", model: expect.objectContaining({ connectionId: "c" }) }));
    });
});

describe("live Recents and persistence", () => {
    it("matches root/directory/title filtering and server ordering", () => {
        const items = [session("a", { title: "MATCH" }), session("z", { title: "match" }), session("new", { title: "match", time: { created: 1, updated: 2 } }), session("child", { title: "match", parentId: "a" }), session("other", { title: "match", directory: "/else" })];
        expect(recentSessions(items, "/work", "match").map((s) => s.id)).toEqual(["new", "z", "a"]);
        expect(mergeSession(items[2], session("new")).time.updated).toBe(2);
    });
    it("applies SSE filtering, removes renamed matches, and preserves active metadata", async () => {
        app.openSession("a"); app.setSearch("match"); await settle();
        const emit = async (s: Session) => { server.emit({ type: "session.updated", data: { info: s } }); await settle(); };
        await emit(session("a", { title: "match" })); await emit(session("child", { title: "match", parentId: "a" }));
        expect(app.sessions.map((s) => s.id)).toEqual(["a"]);
        await emit(session("a", { title: "renamed", time: { created: 1, updated: 2 } }));
        expect(app.sessions).toEqual([]); expect(app.active?.title).toBe("renamed");
    });
    it("does not resurrect an SSE deletion from an in-flight list", async () => {
        const pending = deferred<{ items: Session[]; cursor: { next: undefined } }>(); server.sessions.list.mockReturnValue(pending.promise);
        await vi.advanceTimersByTimeAsync(180); server.emit({ type: "session.deleted", data: { sessionID: "gone" } }); await settle();
        pending.resolve({ items: [session("gone")], cursor: { next: undefined } }); await settle(); expect(app.sessions).toEqual([]);
    });
    it("restores selection by server and encodes safe deep links", () => {
        rememberSession("http://one", "a / b", true); expect(rememberedSession("http://one")).toBe("a / b"); expect(rememberedSession("http://two")).toBeUndefined();
        rememberSession("http://two", "c", true); expect(rememberedSession("http://one")).toBe("a / b");
        rememberSession("http://two", undefined, true); expect(rememberedSession("http://two")).toBeUndefined();
    });
});

const providers = { all: [{ id: 'opencode', name: 'OpenCode', models: {} }, { id: 'openai', name: 'OpenAI', models: { astra: { id: 'gpt-6-astra', name: 'Astra', limit: { context: 372000 } } } }], connected: ['opencode', 'openai'], default: { opencode: 'free', openai: 'gpt-6-astra' } } as any;
const reloadDefaults = () => { app.setPrefs(p => ({ ...p, directory: '/test' })); render(); };
describe('directory defaults and atomic selection precedence', () => {
    it('hydrates actual config model/effort and Build fallback before first submission', async () => {
        const pending = deferred<{ defaultAgent: string | null; model: string | null; variant: string | null }>();
        server.config.defaults.mockReturnValue(pending.promise); reloadDefaults();
        const sending = app.send('first'); await settle();
        expect(server.sessions.create).not.toHaveBeenCalled();
        pending.resolve({ defaultAgent: null, model: 'openai/gpt-6-astra', variant: 'medium' }); await sending; await settle();
        expect(app.model).toBe('openai/gpt-6-astra'); expect(app.agent).toBe('build'); expect(app.thinking).toBe('medium');
        expect(server.sessions.create).toHaveBeenCalledWith(expect.objectContaining({ agent: 'build', model: expect.objectContaining({ id: 'gpt-6-astra', variant: 'medium' }) }));
        expect(server.sessions.update).not.toHaveBeenCalled();
    });
    it('keeps independent health/commands/agents/provider successes when config fails', async () => {
        server.config.defaults.mockRejectedValue(new Error('config unavailable'));
        server.catalog.providers.list.mockResolvedValue(providers);
        server.catalog.agents.list.mockResolvedValue([{ name: 'plan' }] as never); reloadDefaults();
        await settle(); expect(app.connected).toBe(true); expect(app.providerCatalog).toEqual(providers);
        expect(app.model).toBe('openai/gpt-6-astra'); expect(app.models).toHaveLength(1);
        app.onCycleAgent(); render(); expect(app.agent).toBe('plan');
    });
    it('publishes provider catalog even while defaults are pending, but does not submit prematurely', async () => {
        const pending = deferred<any>(); server.config.defaults.mockReturnValue(pending.promise); server.catalog.providers.list.mockResolvedValue(providers); reloadDefaults();
        await settle(); expect(app.models).toHaveLength(1); expect(app.providerCatalog).toEqual(providers);
        pending.resolve({ defaultAgent: 'plan', model: 'custom/config', variant: 'high' }); await settle(); expect(app.model).toBe('custom/config');
    });
    it('keeps defaults when health/provider/commands fail independently', async () => {
        server.config.defaults.mockResolvedValue({ defaultAgent: 'plan', model: 'p/config', variant: 'high' });
        server.health.get.mockRejectedValue(new Error('health')); server.catalog.providers.list.mockRejectedValue(new Error('provider')); server.catalog.commands.list.mockRejectedValue(new Error('commands')); reloadDefaults();
        await settle(); expect(app.model).toBe('p/config'); expect(app.agent).toBe('plan'); expect(app.thinking).toBe('high'); expect(app.connected).toBe(false);
    });
    for (const first of ['config', 'session']) it(`${first}-first completion respects metadata over defaults and per-field explicit choices`, async () => {
        const config = deferred<any>(), metadata = deferred<Session>(); server.config.defaults.mockReturnValue(config.promise); server.sessions.get.mockReturnValue(metadata.promise);
        app.openSession('saved'); render(); app.setThinking(''); render();
        const finishConfig = () => config.resolve({ defaultAgent: 'build', model: 'default/model', variant: 'medium' });
        const finishSession = () => metadata.resolve(session('saved', { agent: 'plan', model: { providerId: 'session', id: 'model', variant: 'high', connectionId: 'account' } }));
        if (first === 'config') { finishConfig(); await settle(); finishSession(); } else { finishSession(); await settle(); finishConfig(); }
        await settle(); expect(app.model).toBe('session/model'); expect(app.agent).toBe('plan'); expect(app.thinking).toBe(''); expect(app.connectionId).toBe('account');
        await app.send('explicit none'); expect(server.sessions.prompt).toHaveBeenCalledWith('saved', expect.objectContaining({ variant: '', model: expect.objectContaining({ variant: '', connectionId: 'account' }) }));
    });
    it('does not let a stale directory default replace the active directory', async () => {
        const old = deferred<any>(); server.config.defaults.mockImplementation(dir => dir === '/new' ? Promise.resolve({ defaultAgent: 'plan', model: 'new/model', variant: 'low' }) : old.promise);
        await settle(); app.setPrefs(p => ({ ...p, directory: '/new' })); await settle(); expect(app.model).toBe('new/model');
        old.resolve({ defaultAgent: 'wrong', model: 'old/model', variant: 'high' }); await settle(); expect(app.model).toBe('new/model'); expect(app.agent).toBe('plan');
    });
    it('does not let old server defaults overwrite the new server', async () => {
        const old = deferred<any>(); server.config.defaults.mockReturnValue(old.promise); await settle();
        const other = client(); other.config.defaults.mockResolvedValue({ defaultAgent: 'plan', model: 'other/model', variant: 'low' }); host.clients.set('http://other', other);
        app.setPrefs(p => ({ ...p, server: 'http://other' })); await settle(); old.resolve({ model: 'wrong/model' }); await settle(); expect(app.model).toBe('other/model');
    });
    it('retains user model choices through hydration without patching defaults', async () => {
        const pending = deferred<any>(); server.config.defaults.mockReturnValue(pending.promise); reloadDefaults(); app.setModel('explicit/model'); app.setAgent('explicit-agent'); render();
        pending.resolve({ defaultAgent: 'plan', model: 'config/model', variant: 'medium' }); await settle();
        expect(app.model).toBe('explicit/model'); expect(app.agent).toBe('explicit-agent'); expect(app.thinking).toBe('medium'); expect(server.sessions.update).not.toHaveBeenCalled();
    });
});
describe('server-scoped tabs, drafts and files', () => {
    it('adds independent local tabs, reuses open sessions, and closes views without delete/abort', async () => {
        const first = app.tabKey; app.onDraftChange('first draft'); app.setThinking('low'); app.newChat(); render();
        const second = app.tabKey; expect(second).not.toBe(first); expect(app.draft).toBe(''); app.onDraftChange('second draft');
        app.activateTab(first); await settle(); expect(app.draft).toBe('first draft'); expect(app.thinking).toBe('low');
        app.openSession('saved'); await settle(); const saved = app.tabKey; app.openSession('saved'); render(); expect(app.tabKey).toBe(saved); expect(app.tabs.filter(t => t.sessionId === 'saved')).toHaveLength(1);
        app.closeTab(saved); render(); expect(app.tabKey).toBe(second); expect(app.draft).toBe('second draft');
        expect(server.sessions.delete).not.toHaveBeenCalled(); expect(server.sessions.abort).not.toHaveBeenCalled();
    });
    it('persists ordered tabs/drafts per server and restores local URL history', async () => {
        const first = app.tabKey; app.onDraftChange('one'); app.newChat(); render(); const second = app.tabKey; app.onDraftChange('two'); render();
        expect(new URLSearchParams(window.location.hash.slice(1)).get('tab')).toBe(second);
        const firstUrl = new URL(window.location.href); firstUrl.hash = new URLSearchParams({ server: app.prefs.server, session: '', tab: first }).toString(); window.location.href = firstUrl.href;
        const restore = vi.mocked(window.addEventListener).mock.calls.find(c => c[0] === 'popstate')![1] as () => void; restore(); render(); expect(app.tabKey).toBe(first); expect(app.draft).toBe('one');
        const other = client(); host.clients.set('http://other', other); app.setPrefs(p => ({ ...p, server: 'http://other' })); await settle(); expect(app.draft).toBe(''); app.onDraftChange('other');
        app.setPrefs(p => ({ ...p, server: 'http://127.0.0.1:4096' })); await settle(); expect(app.tabKey).toBe(first); expect(app.draft).toBe('one'); expect(app.tabs[1].key).toBe(second);
        vi.advanceTimersByTime(0); // Tab writes are coalesced outside input handlers.
        expect(JSON.parse(localStorage.getItem('neoism.gui.tabs:' + app.prefs.server)!).tabs.map((t: any) => t.draft)).toEqual(['one', 'two']);
    });
    it('uploads once to originating session, preserves switched draft/files, and uses captured setters', async () => {
        const file = Object.assign(new Blob(['hello'], { type: 'text/plain' }), { name: 'a.txt' }) as File;
        app.onDraftChange('origin'); app.onFilesChange([file]); render(); const origin = app.tabKey; const oldSetDraft = app.onDraftChange;
        const created = deferred<Session>(); server.sessions.create.mockReturnValue(created.promise);
        const sending = app.send('origin', app.files); await settle(); app.newChat(); render(); app.onDraftChange('other draft'); app.onFilesChange([file]); render();
        created.resolve(session('origin-session')); await sending; render(); expect(app.draft).toBe('other draft'); expect(app.files).toEqual([file]);
        expect(server.artifacts.upload).toHaveBeenCalledTimes(1); expect(server.artifacts.upload).toHaveBeenCalledWith(file, expect.objectContaining({ sessionId: 'origin-session' }));
        expect(server.sessions.prompt).toHaveBeenCalledTimes(1); expect(server.sessions.prompt).toHaveBeenCalledWith('origin-session', expect.objectContaining({ parts: [{ type: 'text', text: 'origin' }, { type: 'file', filename: 'a.txt', mime: 'text/plain', url: 'https://server/artifact/file' }] }));
        oldSetDraft(''); render(); expect(app.draft).toBe('other draft'); app.activateTab(origin); await settle(); expect(app.draft).toBe(''); expect(app.files).toEqual([]);
        vi.advanceTimersByTime(0); // Tab writes are coalesced outside input handlers.
        expect(localStorage.getItem('neoism.gui.tabs:' + app.prefs.server)).not.toContain('"files"');
    });
    it('validates attachment limits before creating or uploading and retains failed drafts', async () => {
        app.onDraftChange('keep'); render(); await expect(app.send('keep', [{ size: 21 * 1024 * 1024 } as File])).rejects.toThrow('limits');
        expect(server.sessions.create).not.toHaveBeenCalled(); expect(server.artifacts.upload).not.toHaveBeenCalled(); expect(app.draft).toBe('keep');
    });
    it('does not clear edits made to the originating draft during a pending prompt', async () => {
        app.onDraftChange('old'); render(); const pending = deferred<void>(); server.sessions.prompt.mockReturnValue(pending.promise); const sending = app.send('old'); await settle();
        app.onDraftChange('new edit'); render(); pending.resolve(); await sending; render(); expect(app.draft).toBe('new edit');
    });
});

describe('pending tab send failure safety', () => {
    it('single-flights an identical attachment send even after switching away and back', async () => {
        const file = Object.assign(new Blob(['data']), { name: 'file.txt' }) as File;
        const pending = deferred<Session>(); server.sessions.create.mockReturnValue(pending.promise);
        const origin = app.tabKey; const first = app.send('hello', [file]); await settle();
        app.newChat(); render(); app.activateTab(origin); render(); const duplicate = app.send('hello', [file]);
        pending.resolve(session('original')); await Promise.all([first, duplicate]);
        expect(server.sessions.create).toHaveBeenCalledTimes(1); expect(server.artifacts.upload).toHaveBeenCalledTimes(1); expect(server.sessions.prompt).toHaveBeenCalledTimes(1);
    });
    it('retains origin draft/files on upload failure and never prompts the switched tab', async () => {
        const file = Object.assign(new Blob(['data']), { name: 'file.txt' }) as File;
        const pending = deferred<any>(); server.artifacts.upload.mockReturnValue(pending.promise);
        app.onDraftChange('retry me'); app.onFilesChange([file]); render(); const origin = app.tabKey;
        const first = app.send('retry me', [file]); const rejected = expect(first).rejects.toThrow('upload failed'); await settle(); app.newChat(); render(); app.onDraftChange('untouched'); render();
        pending.reject(new Error('upload failed')); await rejected; render(); expect(app.draft).toBe('untouched'); expect(server.sessions.prompt).not.toHaveBeenCalled();
        app.activateTab(origin); await settle(); expect(app.draft).toBe('retry me'); expect(app.files).toEqual([file]);
    });
    it('allows an accepted send to finish after closing its view without reopening or aborting', async () => {
        const created = deferred<Session>(); server.sessions.create.mockReturnValue(created.promise); const origin = app.tabKey;
        const sending = app.send('accepted'); await settle(); app.closeTab(origin); render(); const adjacent = app.tabKey;
        created.resolve(session('closed-session')); await sending; render(); expect(app.tabKey).toBe(adjacent); expect(app.id).toBeUndefined(); expect(app.tabs.some(t => t.key === origin)).toBe(false);
        expect(server.sessions.prompt).toHaveBeenCalledWith('closed-session', expect.objectContaining({ prompt: 'accepted' })); expect(server.sessions.delete).not.toHaveBeenCalled(); expect(server.sessions.abort).not.toHaveBeenCalled();
    });
    it('does not send a guessed selection when session metadata hydration fails', async () => {
        server.sessions.get.mockRejectedValue(new Error('metadata failed')); app.openSession('failed'); await settle();
        await expect(app.send('keep')).rejects.toThrow('metadata failed'); expect(server.sessions.prompt).not.toHaveBeenCalled();
    });
});

const serverA = 'http://127.0.0.1:4096';
const serverB = 'http://other';
const transportsFor = (url: string) => host.transports.filter(t => t.baseUrl === url);
describe('server-scoped volatile credentials', () => {
    it('constructs all B transports without A bearer, before health/catalog/defaults execute', async () => {
        app.setToken('secret-A'); await settle(); const other = client(); host.clients.set(serverB, other);
        app.setPrefs(p => ({ ...p, server: serverB })); render();
        expect(app.token).toBe(''); expect(transportsFor(serverB).length).toBeGreaterThan(0); expect(transportsFor(serverB).every(t => !t.token)).toBe(true);
        await settle(); expect(other.health.get).toHaveBeenCalled(); expect(other.config.defaults).toHaveBeenCalled(); expect(other.catalog.providers.list).toHaveBeenCalled();
        app.setToken('secret-B'); await settle(); expect(transportsFor(serverB).at(-1)?.token).toBe('secret-B');
        app.setPrefs(p => ({ ...p, server: serverA })); render(); expect(app.token).toBe('secret-A'); expect(transportsFor(serverA).at(-1)?.token).toBe('secret-A');
        expect(window.location.href).not.toContain('secret-');
        expect(localStorage.getItem('neoism.gui.preferences')).not.toContain('secret-');
        vi.advanceTimersByTime(0); // Tab writes are coalesced outside input handlers.
        expect(localStorage.getItem('neoism.gui.tabs:' + serverA)).not.toContain('secret-');
    });
    it('rejects unchanged source bearer on batched settings save and binds intentional B bearer only to B', async () => {
        const other = client(); host.clients.set(serverB, other); app.setToken('secret-A'); render();
        app.saveSettings({ ...app.prefs, server: serverB }, 'secret-A'); render(); expect(app.token).toBe(''); expect(transportsFor(serverB).every(t => !t.token)).toBe(true);
        app.setPrefs(p => ({ ...p, server: serverA })); render();
        app.saveSettings({ ...app.prefs, server: serverB }, 'intentional-B'); render(); expect(app.token).toBe('intentional-B'); expect(transportsFor(serverB).at(-1)?.token).toBe('intentional-B');
        app.setPrefs(p => ({ ...p, server: serverA })); render(); expect(app.token).toBe('secret-A'); expect(transportsFor(serverA).every(t => t.token !== 'intentional-B')).toBe(true);
    });
    it('keeps a captured setToken scoped to its source even when batched with a server switch', () => {
        const other = client(); host.clients.set(serverB, other); const setAToken = app.setToken;
        app.setPrefs(p => ({ ...p, server: serverB })); setAToken('A-only'); render(); expect(app.token).toBe(''); expect(transportsFor(serverB).every(t => !t.token)).toBe(true);
        app.setPrefs(p => ({ ...p, server: serverA })); render(); expect(app.token).toBe('A-only');
    });
    it('normalizes credential scope but never shares credentials across endpoint paths', () => {
        expect(serverScope('HTTP://EXAMPLE.COM:80/')).toBe(serverScope('http://example.com'));
        expect(serverScope('https://example.com/api/')).not.toBe(serverScope('https://example.com/other/'));
        host.clients.set(serverA + '/', server); app.setToken('normalized-A'); render(); app.setPrefs(p => ({ ...p, server: serverA + '/' })); render(); expect(app.token).toBe('normalized-A');
    });
    it('clears Settings bearer immediately for endpoint edits but not directory edits', async () => {
        const { Settings } = await import('./components/Settings');
        const value = app.prefs; host.cleanup(); host.reset(); vi.stubGlobal('HTMLElement', class {});
        const nodes = (node: any): any[] => !node || typeof node !== 'object' ? [] : Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children)];
        const draw = () => {
            let tree: any, again = true;
            while (again) {
                host.begin(); tree = Settings({ client: server as any, value, token: 'secret-A', save: () => {}, close: () => {} });
                nodes(tree).forEach(n => { if (n.props?.ref && typeof n.props.ref === 'object') n.props.ref.current = { showModal() {}, close() {}, focus() {} }; });
                again = host.flush();
            }
            return nodes(tree);
        };
        let tree = draw(); tree.find(n => n.type === 'button' && Array.isArray(n.props.children) && n.props.children.includes('Servers')).props.onClick(); tree = draw();
        const password = () => tree.find(n => n.type === 'input' && n.props.type === 'password');
        const directory = tree.find(n => n.type === 'input' && n.props.placeholder === 'Server default');
        directory.props.onChange({ target: { value: '/changed' } }); tree = draw(); expect(password().props.value).toBe('secret-A');
        tree.find(n => n.type === 'input' && n.props.type === 'url').props.onChange({ target: { value: serverB } }); tree = draw(); expect(password().props.value).toBe('');
    });
});
describe('connection-specific deletion blocking', () => {
    it('keeps healthy account A usable while deleted B and stale B metadata remain blocked', async () => {
        app.setModel('provider/model'); app.setConnectionId('account-A'); render(); const tabA = app.tabKey;
        app.newChat(); render(); app.setModel('provider/model'); app.setConnectionId('account-B'); render(); const tabB = app.tabKey;
        app.onSelectConnection({ providerId: 'provider', connectionId: 'account-B', label: 'B', reason: 'deleted' }); render();
        await expect(app.send('blocked B')).rejects.toThrow('account was deleted'); expect(server.sessions.create).not.toHaveBeenCalled();
        app.activateTab(tabA); await settle(); await app.send('healthy A'); expect(server.sessions.prompt).toHaveBeenCalledWith('created', expect.objectContaining({ model: expect.objectContaining({ connectionId: 'account-A' }) }));
        app.activateTab(tabB); await settle(); await expect(app.send('still blocked B')).rejects.toThrow('account was deleted');
        server.sessions.get.mockImplementation(async id => session(id, { model: { providerId: 'provider', id: 'model', connectionId: 'account-B' } }));
        app.openSession('stale-B'); await settle(); expect(app.connectionId).toBe('account-B'); await expect(app.send('stale B')).rejects.toThrow('account was deleted');
        expect(server.sessions.prompt).toHaveBeenCalledTimes(1); expect(loadDeletedAccounts(serverA).provider).toContain('account-B');
    });
    it('tracks deletion of a background nonremembered account and scopes tombstones by server', async () => {
        app.setModel('provider/model'); app.setConnectionId('account-A'); render();
        app.onSelectConnection({ providerId: 'provider', connectionId: 'account-B', label: 'B', reason: 'deleted' }); render();
        app.setConnectionId('account-B'); render(); await expect(app.send('deleted background')).rejects.toThrow('account was deleted');
        const other = client(); host.clients.set(serverB, other); app.setPrefs(p => ({ ...p, server: serverB })); await settle(); app.setModel('provider/model'); app.setConnectionId('account-B'); render();
        await app.send('different server'); expect(other.sessions.prompt).toHaveBeenCalled();
    });
    it('blocks unknown defaults after deletion but permits a healthy explicit account despite legacy null', async () => {
        app.setModel('provider/model'); app.setConnectionId('account-B'); render(); app.onSelectConnection({ providerId: 'provider', connectionId: 'account-B', label: 'B', reason: 'deleted' });
        app.newChat(); render(); app.setModel('provider/model'); render(); await expect(app.send('unknown default')).rejects.toThrow('account was deleted');
        server.sessions.get.mockImplementation(async id => session(id, { model: { providerId: 'provider', id: 'model', connectionId: 'account-A' } })); app.openSession('healthy-metadata'); await settle();
        await app.send('explicit metadata'); expect(server.sessions.prompt).toHaveBeenCalledWith('healthy-metadata', expect.objectContaining({ model: expect.objectContaining({ connectionId: 'account-A' }) }));
    });
});
describe('cross-server Back and Forward', () => {
    const pop = (url: string) => {
        window.location.href = url;
        const calls = vi.mocked(window.addEventListener).mock.calls.filter(c => c[0] === 'popstate');
        (calls.at(-1)![1] as () => void)();
    };
    it('switches server before restoring exact session/local keys without corrupting destination URLs or credentials', async () => {
        app.setToken('secret-A'); app.onDraftChange('draft-A'); render(); const localA = app.tabKey, urlLocalA = window.location.href;
        app.openSession('session-A'); await settle(); const sessionA = app.tabKey, urlSessionA = window.location.href;
        const other = client(); host.clients.set(serverB, other); app.saveSettings({ ...app.prefs, server: serverB }, 'secret-B'); await settle(); app.onDraftChange('draft-B'); render(); const localB = app.tabKey, urlLocalB = window.location.href;
        app.openSession('session-B'); await settle(); const sessionB = app.tabKey, urlSessionB = window.location.href;
        for (const [url, endpoint, key, id, draft, credential] of [
            [urlSessionA, serverA, sessionA, 'session-A', '', 'secret-A'],
            [urlLocalA, serverA, localA, undefined, 'draft-A', 'secret-A'],
            [urlLocalB, serverB, localB, undefined, 'draft-B', 'secret-B'],
            [urlSessionB, serverB, sessionB, 'session-B', '', 'secret-B'],
        ] as const) {
            pop(url); expect(window.location.href).toBe(url); await settle();
            expect(app.prefs.server).toBe(endpoint); expect(app.tabKey).toBe(key); expect(app.id).toBe(id); expect(app.draft).toBe(draft); expect(app.token).toBe(credential); expect(window.location.href).toBe(url);
        }
        expect(transportsFor(serverA).every(t => t.token !== 'secret-B')).toBe(true); expect(transportsFor(serverB).every(t => t.token !== 'secret-A')).toBe(true);
    });
});

describe('per-tab project selection', () => {
    it('changes an unsent tab project, rehydrates defaults, persists it and creates nothing until send', async () => {
        server.config.defaults.mockImplementation(async directory => ({ defaultAgent: 'plan', model: directory === '/project-one' ? 'one/model' : 'two/model', variant: directory === '/project-one' ? 'low' : 'high' }));
        const first = app.tabKey; await app.setDirectory('/project-one'); await settle();
        expect(app.directory).toBe('/project-one'); expect(app.model).toBe('one/model'); expect(app.thinking).toBe('low'); expect(app.prefs.directory).toBe('');
        expect(server.sessions.create).not.toHaveBeenCalled(); expect(server.sessions.update).not.toHaveBeenCalled();
        app.newChat(); render(); const second = app.tabKey; await app.setDirectory('/project-two'); await settle(); expect(app.model).toBe('two/model');
        app.activateTab(first); await settle(); expect(app.directory).toBe('/project-one'); expect(app.model).toBe('one/model'); expect(app.tabs.find(t => t.key === second)?.directory).toBe('/project-two');
        vi.advanceTimersByTime(0); // Tab writes are coalesced outside input handlers.
        expect(JSON.parse(localStorage.getItem('neoism.gui.tabs:' + serverA)!).tabs.find((t: any) => t.key === first).directory).toBe('/project-one');
        await app.send('in project one'); expect(server.sessions.create).toHaveBeenCalledWith(expect.objectContaining({ directory: '/project-one', model: expect.objectContaining({ providerId: 'one', id: 'model' }) }));
    });
    it('uses the same setter for /cd without requiring or creating a session', async () => {
        await app.send('/cd /unsent-project'); await settle(); expect(app.directory).toBe('/unsent-project'); expect(app.prefs.directory).toBe(''); expect(server.sessions.create).not.toHaveBeenCalled(); expect(server.sessions.update).not.toHaveBeenCalled();
        app.openSession('existing'); await settle(); await app.setDirectory('/session-project'); await settle(); expect(server.sessions.update).toHaveBeenCalledWith('existing', { directory: '/session-project' }); expect(app.directory).toBe('/session-project'); expect(app.id).toBe('existing');
    });
    it('keeps stale project hydration on its own tab while preserving explicit choices', async () => {
        const pending = deferred<any>(); server.config.defaults.mockImplementation(directory => directory === '/slow' ? pending.promise : Promise.resolve({ defaultAgent: null, model: 'fast/model', variant: 'low' }));
        app.setThinking(''); render(); const origin = app.tabKey; const changing = app.setDirectory('/slow'); render(); app.newChat(); render(); await app.setDirectory('/fast'); await settle();
        pending.resolve({ defaultAgent: 'plan', model: 'slow/model', variant: 'high' }); await changing; await settle(); expect(app.directory).toBe('/fast'); expect(app.model).toBe('fast/model');
        app.activateTab(origin); await settle(); expect(app.directory).toBe('/slow'); expect(app.model).toBe('slow/model'); expect(app.thinking).toBe('');
    });
    it('rejects failed session directory patches and leaves its original project intact', async () => {
        app.openSession('existing'); await settle(); server.sessions.update.mockRejectedValue(new Error('directory denied'));
        await expect(app.setDirectory('/denied')).rejects.toThrow('directory denied'); render(); expect(app.directory).toBe('/work'); expect(server.sessions.create).not.toHaveBeenCalled();
    });
});

describe('final scoped interaction regressions', () => {
    it('opens the directory picker for /cd on an unsent tab without backend session creation', async () => {
        await app.send('/cd'); await settle(); expect(app.picker).toBe('directory'); expect(app.id).toBeUndefined(); expect(server.sessions.create).not.toHaveBeenCalled(); expect(server.operations.request).not.toHaveBeenCalled();
    });
    it('never sends an old upload or prompt through the newly selected server', async () => {
        app.setToken('secret-A'); await settle();
        const upload = deferred<any>(); server.artifacts.upload.mockReturnValue(upload.promise);
        const file = Object.assign(new Blob(['data']), { name: 'a.txt' }) as File;
        const sending = app.send('old upload', [file]); const rejected = expect(sending).rejects.toThrow('Server changed'); await settle(); expect(server.artifacts.upload).toHaveBeenCalledTimes(1);
        const other = client(); host.clients.set(serverB, other); app.setPrefs(p => ({ ...p, server: serverB })); await settle();
        upload.resolve({ filename: 'a.txt', mediaType: 'text/plain', downloadUrl: serverA + '/artifact' }); await rejected;
        expect(other.artifacts.upload).not.toHaveBeenCalled(); expect(other.sessions.prompt).not.toHaveBeenCalled(); expect(server.sessions.prompt).not.toHaveBeenCalled(); expect(transportsFor(serverB).every(t => !t.token)).toBe(true);
    });
});
