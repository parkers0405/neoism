import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
    createNeoismClient,
    createHttpTransport,
    type Session, type ConfigDefaults, type ProviderListResult,
} from "@neoism/sdk";
import { commands as generatedCommands } from "./generated/commands";
import { fonts, systemFontOptions } from "./generated/fonts";
import { themeOptions, appearanceTokens, DEFAULT_GUI_THEME, resolveTheme } from "./appearance";
import { isFxKind, scheduleFx } from "./fx";
import type { ProviderConnectionSelection } from "./providerConnections";
import { loadAccounts, saveAccounts, loadPreferences, rememberedSession, rememberSession, errorMessage, type SlashCommand, type Preferences, serverScope, loadDeletedAccounts, saveDeletedAccounts } from "./types";
import { SessionPinIndex, hydrateSessionPins, isSessionPinned } from "./sessionPins";
import { mergePage, nextCursor } from "./state";
import { nativeCommand } from "./nativeCommands";
import { executeCommand, parseCommand } from "./commands";
import { mergeSession, recentSessions, useSessionEvents } from "./useSessionEvents";
import { useChat } from "./useChat";
import type { Choice } from "./components/Composer";
import { deferredPersistence, messageUsage, modelChoices, useEventCallback } from "./controllerPerformance";
import { groupedModelChoices, loadRecentModels, rememberModel, saveRecentModels } from "./modelRecents";
import { resolveSelection } from "./selection";
import { localTab, loadTabs, saveTabs, closeTab as removeTab, type ChatTab, type TabState } from "./tabs";
export function useAppController() {
    const [prefs, setPrefs] = useState(loadPreferences);
    const tabStores = useRef(new Map<string, TabState>());
    if (!tabStores.current.has(prefs.server)) tabStores.current.set(prefs.server, loadTabs(prefs.server));
    const [, refreshTabs] = useState(0);
    const tabState = tabStores.current.get(prefs.server)!;
    const tab = tabState.tabs.find(t => t.key === tabState.active)!;
    const tabKey = tab.key;
    const persistence = useMemo(() => deferredPersistence(saveTabs), []);
    useEffect(() => {
        window.addEventListener('pagehide', persistence.flush);
        return () => { window.removeEventListener('pagehide', persistence.flush); persistence.flush(); };
    }, [persistence]);
    const persistTabs = (server = prefs.server) => { persistence.schedule(server, tabStores.current.get(server)!); refreshTabs(n => n + 1); };
    // Resolve synchronously before constructing any transport. An effect that
    // clears a global token would already have leaked it to the new endpoint.
    const credentials = useRef(new Map<string, string>());
    const [, credentialsChanged] = useState(0);
    const credentialScope = serverScope(prefs.server);
    const token = credentials.current.get(credentialScope) || "";
    const setToken = (value: string) => {
        credentials.current.set(credentialScope, value);
        credentialsChanged(n => n + 1);
    };
    const saveSettings = (next: Preferences, suppliedToken: string) => {
        const destination = serverScope(next.server);
        // Defend old/batched hosts as well as the Settings UI: an unchanged
        // source credential is not consent to send it to a different endpoint.
        if (destination === credentialScope || suppliedToken !== token || !suppliedToken)
            credentials.current.set(destination, suppliedToken);
        setPrefs(next);
        credentialsChanged(n => n + 1);
    };
    const [settings, setSettings] = useState(false);
    const [view, setView] = useState<"chat" | "skills" | "workflows">("chat");
    const [id, updateId] = useState<string>();
    const selectedId = useRef<string | undefined>(undefined);
    const selectionEpoch = useRef(0);
    const [active, setActive] = useState<Session>();
    const [children, setChildren] = useState<Session[]>([]);
    const setId = useCallback((value: string | undefined) => {
        selectionEpoch.current++;
        selectedId.current = value;
        updateId(value);
        setActive(undefined);
        setChildren([]);
    }, []);
    const [sessions, setSessions] = useState<Session[]>([]);
    const [cursor, setCursor] = useState<string>();
    const [search, setSearch] = useState("");
    const listScope = `${credentialScope}:${token}:${prefs.directory}:${search}`;
    const currentListScope = useRef(listScope);
    currentListScope.current = listScope;
    const [loadedListScope, setLoadedListScope] = useState(listScope);
    const [listBusy, setListBusy] = useState(true);
    const [error, setError] = useState("");
    const [connected, setConnected] = useState(false);
    const [sidebar, setSidebarState] = useState(() => {
        try {
            const saved = localStorage.getItem("neoism.gui.details-visible");
            if (saved === "true" || saved === "false") return saved === "true";
        } catch {}
        return false;
    });
    const setSidebar = useCallback((next: boolean | ((previous: boolean) => boolean)) => {
        setSidebarState(previous => {
            const value = typeof next === "function" ? next(previous) : next;
            try { localStorage.setItem("neoism.gui.details-visible", String(value)); } catch {}
            return value;
        });
    }, []);
    const [nav, setNav] = useState(false);
    const [hints, setHints] = useState(true);
    const [info, setInfo] = useState<{ title: string; body: string }>();
    const [dynamic, setDynamic] = useState<SlashCommand[]>([]);
    const [models, setModels] = useState<Choice[]>([]);
    const [agents, setAgents] = useState<Choice[]>([]);
    const [model, updateModel] = useState("");
    const [agent, updateAgent] = useState("");
    const [thinking, updateThinking] = useState("");
    const [connectionId, updateConnectionId] = useState("");
    const accounts = useMemo(() => loadAccounts(prefs.server), [prefs.server]);
    const deletedAccountStores = useRef(new Map<string, Record<string, string[]>>());
    if (!deletedAccountStores.current.has(credentialScope)) deletedAccountStores.current.set(credentialScope, loadDeletedAccounts(prefs.server));
    const deletedAccounts = deletedAccountStores.current.get(credentialScope)!;
    const [catalogRevision, setCatalogRevision] = useState(0);
    const [directories, setDirectories] = useState<Choice[]>([]);
    const [directoryQuery, setDirectoryQuery] = useState("");
    const [skills, setSkills] = useState<Choice[]>([]);
    const [draftInsertion, setDraftInsertion] = useState<{ text: string; revision: number }>();
    const [picker, setPicker] = useState<string>();
    const togglePicker = useCallback((kind: string) => setPicker(old => old === kind ? undefined : kind), []);
    const recentModelStores = useRef(new Map<string, string[]>());
    if (!recentModelStores.current.has(credentialScope)) recentModelStores.current.set(credentialScope, loadRecentModels(prefs.server));
    const recentModels = recentModelStores.current.get(credentialScope)!;
    const [skipState, setSkipState] = useState<{
        session?: string;
        server?: string;
        enabled: boolean;
    }>({ enabled: false });
    const skipPermissions =
        skipState.session === id &&
        skipState.server === prefs.server &&
        skipState.enabled;
    const setSkipPermissions = (
        value: boolean | ((prior: boolean) => boolean),
    ) =>
        setSkipState((prior) => ({
            session: id,
            server: prefs.server,
            enabled:
                typeof value === "function"
                    ? value(
                          prior.session === id &&
                              prior.server === prefs.server &&
                              prior.enabled,
                      )
                    : value,
        }));
    const [effect, setEffect] = useState("");
    const [effectRevision, setEffectRevision] = useState(0);
    const fxScope = useRef<{ epoch: number; client: unknown; directory: string; preferenceDirectory: string } | undefined>(undefined);
    useEffect(() => {
        setSkipPermissions(false);
    }, [id, prefs.server, token]);
    const notify = useCallback((message: string) => setError(message), []);
    const client = useMemo(
        () =>
            createNeoismClient(
                createHttpTransport({
                    baseUrl: prefs.server,
                    token: token || undefined,
                }),
            ),
        [prefs.server, token],
    );
    const pinIndex = useMemo(() => new SessionPinIndex(prefs.server), [client]);
    const pinFlights = useMemo(() => new Set<string>(), [client]);
    const deletedSessions = useMemo(() => new Set<string>(), [client]);
    const currentClient = useRef(client);
    currentClient.current = client;
    useEffect(() => {
        setId(undefined);
        setError("");
        setInfo(undefined);
        setPicker(undefined);
        updateModel("");
        updateAgent("");
        updateThinking("");
        updateConnectionId("");
        setModels([]);
        setProviderCatalog(undefined);
        setAgents([]);
        setDynamic([]);
        return () => {
            selectionEpoch.current++;
        };
    }, [client, setId]);
    useEffect(() => {
        if (!selectedId.current) {
            selectionEpoch.current++;
        }
    }, [prefs.directory]);
    const onSession = useCallback((session: Session | string) => {
        if (currentClient.current !== client) return;
        if (typeof session === "string") deletedSessions.add(session);
        else {
            if (deletedSessions.has(session.id)) return;
            session = mergeSession(liveSessions.current.get(session.id) || undefined, session);
        }
        pinIndex.observe(session);
        liveSessions.current.set(typeof session === "string" ? session : session.id, typeof session === "string" ? null : session);
        const store = tabStores.current.get(prefs.server);
        if (store && typeof session !== "string") {
            store.tabs.forEach(t => { if (t.sessionId === session.id) t.metadata = mergeSession(t.metadata, session); });
            persistence.schedule(prefs.server, store); refreshTabs(n => n + 1);
        }
        if (typeof session === "string") {
            if (store) {
                const removed = store.tabs.filter(t => t.sessionId === session);
                for (const target of removed) {
                    const next = removeTab(store, target.key); store.tabs = next.tabs;
                    if (store.active !== next.active) activateTab(next.active, false);
                }
                persistence.schedule(prefs.server, store); refreshTabs(n => n + 1);
            }
            setChildren((old) => old.filter((s) => s.id !== session));
        } else {
            if (selectedId.current === session.id)
                setActive((old) => mergeSession(old, session));
            setChildren((old) => old.map((s) => s.id === session.id ? mergeSession(s, session) : s));
        }
    }, [setId, prefs.server, prefs.directory, client]);
    useSessionEvents(client, prefs.directory, search, setSessions, notify, onSession);
    const chat = useChat(client, id, notify);
    const listEpoch = useRef(0);
    const listFlight = useRef(false);
    const recentFilter = useRef({ directory: prefs.directory, search });
    recentFilter.current = { directory: prefs.directory, search };
    const liveSessions = useRef(new Map<string, Session | null>());
    const reconcileList = (items: Session[]) => {
        const map = new Map(items.map((s) => [s.id, s]));
        liveSessions.current.forEach((s, id) => {
            if (s) map.set(id, mergeSession(map.get(id), s));
            else map.delete(id);
        });
        deletedSessions.forEach(id => map.delete(id));
        map.forEach(s => pinIndex.observe(s));
        return recentSessions([...map.values()], recentFilter.current.directory, recentFilter.current.search);
    };
    const catalog = useMemo(
        () =>
            mergePage(
                dynamic,
                generatedCommands.map((c) => ({
                    ...c,
                    aliases: [...c.aliases],
                })),
                (c) => c.name.replace(/^\//, ""),
            ),
        [dynamic],
    );
    useEffect(() => {
        if (prefs.theme === "neoism") setPrefs(p => ({...p, theme: DEFAULT_GUI_THEME}));
        const theme = resolveTheme(prefs.theme);
        const root = document.documentElement;
        const appearance = appearanceTokens(theme);
        root.style.colorScheme = appearance.colorScheme;
        for (const key of Object.keys(themeOptions[1].colors)) {
            if (!(key in theme.colors)) root.style.removeProperty("--theme-" + key);
        }
        Object.entries(theme.colors).forEach(([key, value]) =>
            root.style.setProperty("--theme-" + key, value),
        );
        Object.entries(appearance.variables).forEach(([key, value]) =>
            root.style.setProperty("--" + key, value),
        );
        document.documentElement.style.setProperty(
            "--font-heading",
            fonts.find((font) => font.id === "geist")?.cssFamily || "Geist, ui-sans-serif, system-ui, sans-serif",
        );
        document.documentElement.style.setProperty(
            "--font",
            [...fonts, ...systemFontOptions].find((f) => f.id === prefs.font)
                ?.cssFamily || "Geist, ui-sans-serif, system-ui, sans-serif",
        );
        document.documentElement.style.setProperty(
            "--font-code",
            [...fonts, ...systemFontOptions].find((f) => f.id === (prefs.codeFont || "jetbrains-mono"))
                ?.cssFamily || '"Neoism JetBrains Mono", ui-monospace, monospace',
        );
        try {
            localStorage.setItem(
                "neoism.gui.preferences",
                JSON.stringify(prefs),
            );
        } catch {
            notify(
                "Browser storage is unavailable. Settings apply for this visit but cannot be saved.",
            );
        }
    }, [prefs]);
    const tabDirectory = (target: ChatTab) => target.metadata?.directory ?? target.directory ?? prefs.directory;
    const directory = tabDirectory(tab);
    const currentDirectory = useRef(directory);
    currentDirectory.current = directory;
    const [providerCatalog, setProviderCatalog] = useState<ProviderListResult>();
    const catalogScope = `${credentialScope}:${token}:${directory}`;
    const [catalogPending, setCatalogPending] = useState({ scope: '', model: false, agent: false });
    const defaultsCache = useRef(new Map<string, { defaults?: ConfigDefaults; providers?: ProviderListResult; models?: Choice[]; promise: Promise<void> }>());
    const hydrationKey = (dir: string) => `${prefs.server}:${token}:${dir}`;
    const hydrate = (dir: string) => {
        const key = hydrationKey(dir);
        const existing = defaultsCache.current.get(key);
        if (existing) return existing;
        const entry: { defaults?: ConfigDefaults; providers?: ProviderListResult; models?: Choice[]; promise: Promise<void> } = { promise: Promise.resolve() };
        const request = <T,>(fn: () => Promise<T>, apply: (v: T) => void) => Promise.resolve().then(fn).then(apply).catch(e => { if (currentClient.current === client && currentDirectory.current === dir) notify(errorMessage(e)); });
        entry.promise = Promise.all([
            request(() => client.config.defaults(dir || undefined), v => { entry.defaults = v; }),
            request(() => client.catalog.providers.list(dir), v => {
                entry.providers = v;
                if (currentClient.current === client && currentDirectory.current === dir) {
                    setProviderCatalog(v);
                }
            }),
            request(() => client.catalog.providers.configured(dir), v => {
                entry.models = modelChoices({ all: v.providers, connected: v.providers.map(p => p.id), default: v.default });
                if (currentClient.current === client && currentDirectory.current === dir) setModels(entry.models);
            }).finally(() => {
                if (currentClient.current === client && currentDirectory.current === dir) setCatalogPending(p => ({ ...p, model: false }));
            }),
        ]).then(() => {});
        defaultsCache.current.set(key, entry);
        return entry;
    };
    const resolved = (target: ChatTab, data: { defaults?: ConfigDefaults; providers?: ProviderListResult }) => {
        const next = resolveSelection(target.explicit, target.metadata, data.defaults, data.providers);
        if (target.explicit.connectionId === undefined && !target.metadata?.model?.connectionId) next.connectionId = accounts[next.model.split('/')[0]] || '';
        return next;
    };
    const applySelection = (target: ChatTab, dir: string) => {
        const data = defaultsCache.current.get(hydrationKey(dir));
        const next = resolved(target, data || {});
        selection.current = next;
        updateModel(next.model); updateAgent(next.agent); updateThinking(next.thinking); updateConnectionId(next.connectionId);
        return next;
    };
    const refreshCatalog = useRef('');
    useEffect(() => {
        let alive = true;
        const refreshKey = `${settings}:${catalogRevision}:${picker === 'connect'}`;
        if (refreshCatalog.current !== refreshKey) { defaultsCache.current.delete(hydrationKey(directory)); refreshCatalog.current = refreshKey; }
        setConnected(false);
        if (catalogPending.scope !== catalogScope) { setModels([]); setAgents([]); }
        setCatalogPending({ scope: catalogScope, model: true, agent: true });
        const request = <T,>(fn: () => Promise<T>, apply: (v: T) => void) => { void Promise.resolve().then(fn).then(v => { if (alive) apply(v); }).catch(e => { if (alive) notify(errorMessage(e)); }); };
        request(() => client.health.get(), () => setConnected(true));
        request(() => client.catalog.commands.list(directory), commands => setDynamic(commands.map(c => ({ name: c.name, description: c.description || "Server command", aliases: [] }))));
        request(() => client.catalog.agents.list(directory).finally(() => { if (alive) setCatalogPending(p => ({ ...p, agent: false })); }), agents => setAgents(agents
            .filter(a => !a.hidden && a.mode !== "subagent")
            .map(a => ({ id: a.name, label: a.name, description: a.description || '' }))));
        const entry = hydrate(directory);
        void entry.promise.then(() => {
            if (!alive) return;
            setProviderCatalog(entry.providers);
            setModels(entry.models || []);
            setCatalogPending(p => ({ ...p, model: false }));
            // The tab may have changed while directory hydration was in flight.
            const selected = tabState.tabs.find(t => t.key === tabState.active);
            if (selected) applySelection(selected, directory);
        });
        return () => { alive = false; };
    }, [client, directory, notify, settings, catalogRevision, picker === "connect"]);
    useEffect(() => {
        const generation = ++listEpoch.current;
        liveSessions.current.clear();
        listFlight.current = false;
        setLoadedListScope(listScope);
        setListBusy(true);
        setSessions([]);
        setCursor(undefined);
        const alive = () => generation === listEpoch.current && currentClient.current === client && currentListScope.current === listScope;
        void hydrateSessionPins(client, pinIndex, alive, session => {
            // A newer mutation/event (including deletion) always wins over hydration.
            if (liveSessions.current.has(session.id) || deletedSessions.has(session.id)) return;
            pinIndex.observe(session);
            setSessions(old => reconcileList(isSessionPinned(session) || old.some(s => s.id === session.id)
                ? [...old.filter(s => s.id !== session.id), session] : old));
        });
        const timer = setTimeout(() => {
            void client.sessions
                .list({
                    roots: true,
                    limit: 30,
                    search: search || undefined,
                    directory: prefs.directory || undefined,
                })
                .then((page) => {
                    if (!alive()) return;
                    setSessions(old => reconcileList(mergePage(page.items, old, s => s.id)));
                    setCursor(page.cursor.next);
                })
                .catch((e) => {
                    if (alive()) notify(errorMessage(e));
                })
                .finally(() => {
                    if (alive()) setListBusy(false);
                });
        }, 180);
        return () => {
            clearTimeout(timer);
            listEpoch.current++;
        };
    }, [client, prefs.directory, search, notify]);
    const recentMore = async () => {
        if (currentClient.current !== client || currentListScope.current !== listScope || !cursor || listBusy || listFlight.current) return;
        listFlight.current = true;
        setListBusy(true);
        const generation = listEpoch.current;
        try {
            const page = await client.sessions.list({
                roots: true,
                limit: 30,
                cursor,
                search: search || undefined,
                directory: prefs.directory || undefined,
            });
            if (generation !== listEpoch.current || currentClient.current !== client) return;
            setSessions((s) => reconcileList(mergePage(page.items, s, (x) => x.id)));
            setCursor(nextCursor(cursor, page.cursor.next));
        } catch (e) {
            if (generation === listEpoch.current && currentClient.current === client) notify(errorMessage(e));
        } finally {
            if (generation === listEpoch.current && currentClient.current === client) {
                listFlight.current = false;
                setListBusy(false);
            }
        }
    };
    const metadataFlights = useRef(new Map<string, Promise<void>>());
    const creations = useRef(new Map<string, Promise<string>>());
    const activateTab = (key: string, push = true) => {
        const target = tabState.tabs.find(t => t.key === key);
        if (!target) return;
        tabState.active = key; persistTabs();
        setId(target.sessionId); setActive(target.metadata);
        applySelection(target, tabDirectory(target));
        rememberSession(prefs.server, target.sessionId, push, target.key);
        setView('chat'); setNav(false);
        if (target.sessionId) {
            const sessionId = target.sessionId;
            const pending = client.sessions.get(sessionId).then(s => {
                if (currentClient.current !== client || metadataFlights.current.get(key) !== pending) return;
                target.metadata = mergeSession(target.metadata, s); persistTabs();
                if (tabState.active !== key) return;
                setActive(target.metadata);
                applySelection(target, tabDirectory(target));
            });
            void pending.catch(e => { if (currentClient.current === client && tabState.active === key) notify(errorMessage(e)); });
            metadataFlights.current.set(key, pending);
        }
    };
    const openSession = useEventCallback((sessionId: string, push: boolean = true) => {
        let target = tabState.tabs.find(t => t.sessionId === sessionId);
        if (!target) {
            target = { ...localTab(), sessionId, metadata: sessions.find(s => s.id === sessionId) || children.find(s => s.id === sessionId) };
            tabState.tabs.push(target);
        }
        activateTab(target.key, push);
    });
    useEffect(() => {
        const restore = (fromHistory = false) => {
            const params = new URLSearchParams(window.location.hash.slice(1));
            const destination = params.get('server');
            if (fromHistory && destination && destination !== prefs.server) {
                // Do not activate/replace the source URL. The destination
                // client's effect restores this same untouched history entry.
                setPrefs(prior => ({ ...prior, server: destination }));
                return;
            }
            const key = destination === prefs.server ? params.get('tab') : null;
            if (key && tabState.tabs.some(t => t.key === key)) activateTab(key, false);
            else { const session = rememberedSession(prefs.server); if (session) openSession(session, false); else activateTab(tabState.active, false); }
        };
        const onHistory = () => restore(true);
        restore(); window.addEventListener('popstate', onHistory);
        return () => window.removeEventListener('popstate', onHistory);
    }, [client]);
    const newChat = () => { const target = localTab(); tabState.tabs.push(target); activateTab(target.key); };
    const closeTab = (key: string) => {
        const next = removeTab(tabState, key); tabState.tabs = next.tabs;
        if (tabState.active !== next.active) activateTab(next.active);
        else persistTabs();
    };
    const wireVariant = (target: ChatTab, thinking: string) => target.explicit.thinking !== undefined || target.metadata?.model ? thinking : thinking || undefined;
    const prepare = async (target: ChatTab): Promise<ReturnType<typeof resolveSelection>> => {
        let flight: Promise<void> | undefined;
        do { flight = metadataFlights.current.get(target.key); await flight; } while (flight !== metadataFlights.current.get(target.key));
        const dir = tabDirectory(target);
        const data = hydrate(dir); await data.promise;
        if (tabDirectory(target) !== dir) return prepare(target);
        if (currentClient.current !== client) throw new Error('Server changed. Submit again in the current chat.');
        return resolved(target, data);
    };
    const ensureSession = async (target: ChatTab = tabState.tabs.find(t => t.key === tabState.active)!): Promise<string> => {
        const next = await prepare(target);
        if (target.sessionId) return target.sessionId;
        const existing = creations.current.get(target.key); if (existing) return existing;
        assertAccount(next.model, next.connectionId);
        const slash = next.model.indexOf('/');
        const pending = client.sessions.create({ directory: tabDirectory(target) || undefined, agent: next.agent || undefined,
            ...(slash > 0 ? { model: { providerId: next.model.slice(0, slash), id: next.model.slice(slash + 1), variant: wireVariant(target, next.thinking), connectionId: next.connectionId || undefined } } : {}),
        }).then(s => {
            target.sessionId = s.id; target.metadata = s; persistTabs();
            if (currentClient.current === client) {
                setSessions(old => recentSessions([...old.filter(x => x.id !== s.id), s], recentFilter.current.directory, recentFilter.current.search));
                if (tabState.active === target.key) { chat.markCreatedSession(s.id); selectedId.current = s.id; updateId(s.id); setActive(s); rememberSession(prefs.server, s.id, false, target.key); }
            }
            return s.id;
        }).finally(() => creations.current.delete(target.key));
        creations.current.set(target.key, pending); return pending;
    };
    const selection = useRef({ model, agent, thinking, connectionId });
    selection.current = { model, agent, thinking, connectionId };
    const patchQueues = useRef(new Map<string, Promise<unknown>>());
    const patchSession = (patch: Parameters<typeof client.sessions.update>[1], rethrow = false) => {
        const target = tab;
        const session = target.sessionId;
        if (!session) return Promise.resolve();
        const current = () => currentClient.current === client;
        const request = (patchQueues.current.get(target.key) || Promise.resolve()).catch(() => {}).then(async () => {
            if (!current()) return;
            const updated = await client.sessions.update(session, patch);
            if (!current()) return;
            target.metadata = mergeSession(target.metadata, updated); persistTabs();
            if (tabState.active === target.key) setActive(old => mergeSession(old, updated));
            setSessions(old => recentSessions([...old.filter(s => s.id !== updated.id), updated], recentFilter.current.directory, recentFilter.current.search));
        });
        patchQueues.current.set(target.key, request);
        return request.catch(e => { if (current() && tabState.active === target.key) notify(errorMessage(e)); if (rethrow) throw e; });
    };
    const setDirectory = async (value: string): Promise<void> => {
        const target = tab;
        if (currentClient.current !== client) throw new Error("Server changed. Choose the project again.");
        if (target.sessionId) await patchSession({ directory: value }, true);
        else {
            if (creations.current.has(target.key)) throw new Error("Wait for chat creation to finish before changing its project.");
            target.directory = value;
            persistTabs();
        }
        const dir = tabDirectory(target);
        await hydrate(dir).promise;
        if (currentClient.current === client && tabState.active === target.key && tabDirectory(target) === dir) applySelection(target, dir);
    };
    const select = (key: "model" | "agent" | "thinking" | "connectionId", value: string) => {
        tab.explicit[key] = value;
        const next = { ...selection.current, [key]: value };
        if (key === "model" && next.model.split("/")[0] !== selection.current.model.split("/")[0]) next.connectionId = accounts[next.model.split("/")[0]] || "";
        tab.explicit = { ...tab.explicit, [key]: value, ...(key === "model" ? { connectionId: next.connectionId } : {}) }; persistTabs();
        selection.current = next;
        updateModel(next.model);
        updateAgent(next.agent);
        updateThinking(next.thinking);
        updateConnectionId(next.connectionId);
        if (!tab.sessionId) return;
        // Resolve the whole model context before writing a user action. In
        // particular an effort click during metadata loading must not PATCH {}.
        void prepare(tab).then(hydrated => {
            const slash = hydrated.model.indexOf('/');
            if (key !== 'agent') assertAccount(hydrated.model, hydrated.connectionId);
            return patchSession(key === 'agent' ? { agent: value } : slash > 0 ? {
                model: { providerId: hydrated.model.slice(0, slash), id: hydrated.model.slice(slash + 1), variant: hydrated.thinking, connectionId: hydrated.connectionId || undefined },
            } : {});
        }).catch(e => { if (currentClient.current === client && tabState.active === tab.key) notify(errorMessage(e)); });
    };
    const setModel = (value: string) => {
        const next = rememberModel(recentModelStores.current.get(credentialScope) || [], value);
        recentModelStores.current.set(credentialScope, next);
        saveRecentModels(prefs.server, next);
        select("model", value);
    };
    const setAgent = (value: string) => select("agent", value);
    const setThinking = (value: string) => select("thinking", value);
    const setConnectionId = (value: string) => {
        const provider = selection.current.model.split("/")[0];
        if (provider) { accounts[provider] = value; saveAccounts(prefs.server, accounts); }
        select("connectionId", value);
    };
    const assertAccount = (model: string, connection: string) => {
        const provider = model.split("/")[0];
        if ((connection && deletedAccounts[provider]?.includes(connection)) || (!connection && accounts[provider] === null))
            throw new Error("The selected account was deleted. Use /connect to explicitly choose an account before sending.");
    };
    const onSelectConnection = (event: ProviderConnectionSelection) => {
        if (currentClient.current !== client) return;
        const provider = selection.current.model.split("/")[0];
        if (event.reason === "deleted") {
            deletedAccounts[event.providerId] = [...new Set([...(deletedAccounts[event.providerId] || []), event.connectionId])];
            saveDeletedAccounts(prefs.server, deletedAccounts);
            if (accounts[event.providerId] === event.connectionId || (provider === event.providerId && selection.current.connectionId === event.connectionId)) {
                accounts[event.providerId] = null;
                saveAccounts(prefs.server, accounts);
                notify("The selected account was deleted. Choose an account explicitly before sending.");
            }
            return;
        }
        accounts[event.providerId] = event.connectionId;
        saveAccounts(prefs.server, accounts);
        if (provider !== event.providerId) {
            const first = models.find((m) => m.id.startsWith(event.providerId + "/"));
            if (first) setModel(first.id);
            else { setPicker("model"); setCatalogRevision((old) => old + 1); return; }
        }
        setConnectionId(event.connectionId);
        setPicker(undefined);
        setCatalogRevision((old) => old + 1);
    };
    const pinSession = async (sessionId: string, pinned: boolean) => {
        if (currentClient.current !== client || pinFlights.has(sessionId)) return;
        pinFlights.add(sessionId);
        try {
            const updated = await client.sessions.pin(sessionId, pinned);
            if (currentClient.current !== client || deletedSessions.has(sessionId)) return;
            onSession(updated);
            setSessions(old => reconcileList([...old.filter(s => s.id !== sessionId), updated]));
        } catch (e) { if (currentClient.current === client) notify(errorMessage(e)); }
        finally { pinFlights.delete(sessionId); }
    };
    const renameSession = async (sessionId: string, title: string) => {
        if (currentClient.current !== client || !title.trim()) return;
        try {
            const updated = await client.sessions.update(sessionId, { title });
            if (currentClient.current !== client || deletedSessions.has(sessionId)) return;
            onSession(updated);
            setSessions((old) => reconcileList([...old.filter((s) => s.id !== sessionId), updated]));
        } catch (e) { if (currentClient.current === client) notify(errorMessage(e)); }
    };
    const deleteSession = async (sessionId: string) => {
        if (currentClient.current !== client) return;
        try {
            await client.sessions.delete(sessionId);
            if (currentClient.current !== client) return;
            onSession(sessionId);
            setSessions((old) => old.filter((s) => s.id !== sessionId));
        } catch (e) { if (currentClient.current === client) notify(errorMessage(e)); }
    };
    const insertSkill = (name: string) => {
        const old = tab.draft.startsWith('/skill') ? '' : tab.draft;
        tab.draft = `${old}${old && !/\s$/.test(old) ? ' ' : ''}$${name} `; persistTabs();
        setDraftInsertion(prior => ({ text: `$${name} `, revision: (prior?.revision || 0) + 1 }));
    };
    const perform = async (fn: () => Promise<unknown>) => {
        try {
            await fn();
        } catch (e) {
            notify(errorMessage(e));
        }
    };
    const showResult = (title: string, result: unknown) =>
        setInfo({
            title,
            body:
                typeof result === "string"
                    ? result
                    : JSON.stringify(result, null, 2),
        });
    const promptSession = async (text: string, guard: () => boolean = () => true, files: File[] = []) => {
        if (!guard()) return;
        if (files.length > 10 || files.some(f => f.size > 20 * 1024 * 1024) || files.reduce((n, f) => n + f.size, 0) > 50 * 1024 * 1024) throw new Error('Attachments exceed limits (10 files, 20 MB each, 50 MB total).');
        const target = tab;
        const originalDraft = target.draft;
        const session = await ensureSession(target);
        if (!guard()) return;
        const { model, agent, thinking, connectionId } = await prepare(target);
        assertAccount(model, connectionId);
        const parts = await Promise.all(files.map(async file => {
            const artifact = await client.artifacts.upload(file, { filename: file.name, mediaType: file.type || 'application/octet-stream', sessionId: session });
            return { type: 'file' as const, filename: artifact.filename, mime: artifact.mediaType, url: artifact.downloadUrl };
        }));
        if (currentClient.current !== client) throw new Error('Server changed. Try again.');
        assertAccount(model, connectionId);
        const slash = model.indexOf('/');
        await client.sessions.prompt(session, {
            messageId: chat.beginPrompt(session),
            ...(parts.length ? { parts: [{ type: 'text' as const, text }, ...parts] } : { prompt: text }), agent: agent || undefined, author: prefs.name, variant: wireVariant(target, thinking),
            ...(slash > 0 ? { model: { providerId: model.slice(0, slash), modelId: model.slice(slash + 1), variant: wireVariant(target, thinking), connectionId: connectionId || undefined } } : {}),
        });
        if (target.draft === originalDraft) target.draft = '';
        target.files = target.files?.filter(f => !files.includes(f)); persistTabs();
        if (currentClient.current === client && tabState.active === target.key) chat.setState(s => ({ ...s, busy: true }), session);
    };
    useEffect(() => {
        if (!isFxKind(effect)) return;
        const scope = fxScope.current;
        const current = () => !!scope && scope === fxScope.current && scope.client === currentClient.current && scope.epoch === selectionEpoch.current && scope.directory === currentDirectory.current && scope.preferenceDirectory === recentFilter.current.directory;
        if (!current()) { setEffect(""); return; }
        return scheduleFx(effect, (text) => {
            void promptSession(text, current).catch((e) => { if (current()) notify(errorMessage(e)); });
        }, () => setEffect(""), current);
    }, [effect, effectRevision, client, prefs.directory, directory, selectionEpoch.current]);
    const runNative = async (name: string, args: string) => {
        const generation = selectionEpoch.current;
        const current = () => currentClient.current === client && generation === selectionEpoch.current;
        const session = name === "goal" && args && !["clear", "pause", "resume"].includes(args)
            ? await ensureSession() : selectedId.current;
        if (!current()) throw new Error("Chat changed. Try again.");
        const context = {
            client, id: session, directory: directory,
            show: (title: string, value: unknown) => { if (current()) showResult(title, value); },
            ensureSession: async () => {
                if (!current()) throw new Error("Chat changed. Try again.");
                return ensureSession();
            },
            sendPrompt: async (sessionId: string, text: string) => {
                if (!current() || selectedId.current !== sessionId) throw new Error("Chat changed. Try again.");
                await promptSession(text);
            },
        };
        return nativeCommand(name, args, context);
    };
    const local = async (name: string, args: string): Promise<boolean> => {
        const generation = selectionEpoch.current;
        const current = () => currentClient.current === client && generation === selectionEpoch.current;
        const show = (title: string, result: unknown) => { if (current()) showResult(title, result); };
        switch (name) {
            case "help":
                setInfo({
                    title: "Commands",
                    body: catalog
                        .map(
                            (c) =>
                                "/" +
                                c.name.replace(/^\//, "") +
                                " — " +
                                c.description,
                        )
                        .join("\n"),
                });
                return true;
            case "model":
            case "models":
                if (args) {
                    const match = models.find(
                        (m) => m.id === args || m.id.endsWith("/" + args),
                    );
                    if (!match)
                        throw new Error(
                            "Model not found. Use /models to choose a server model.",
                        );
                    setModel(match.id);
                } else setPicker("model");
                return true;
            case "agent":
                if (args) {
                    if (!agents.some((a) => a.id === args))
                        throw new Error("Unknown agent. Use /agent to choose.");
                    setAgent(args);
                } else setPicker("agent");
                return true;
            case "think":
            case "reasoning":
                if (args) setThinking(args);
                else setPicker("thinking");
                return true;
            case "sessions":
            case "session":
                if (args) openSession(args);
                else setPicker("sessions");
                return true;
            case "connect":
                setPicker("connect");
                return true;
            case "settings":
                setSettings(true);
                return true;
            case "cd":
                if (args) await setDirectory(args);
                else { setDirectoryQuery(""); setPicker("directory"); }
                return true;
            case "skill":
            case "skills":
                if (/^(list|info)(\s|$)/.test(args)) return runNative(name, args);
                if (args) {
                    const available = await client.catalog.skills.list(directory);
                    if (!current()) return true;
                    if (!available.some((s) => s.name === args)) throw new Error("Unknown skill. Use /skill to choose.");
                    insertSkill(args);
                } else setPicker("skill");
                return true;
            case "new":
                newChat();
                return true;
            case "exit":
                newChat();
                return true;
            case "hints":
            case "helper":
            case "help-strip":
            case "footer":
                setHints((x) => !x);
                return true;
            case "sidebar":
                setSidebar((x) => !x);
                return true;
            case "sub-agent":
            case "subagents":
            case "sub":
                setPicker("subagents");
                return true;
            case "undo":
            case "redo":
            case "abort":
            case "compact":
            case "compaction":
            case "comapction": {
                if (!id) throw new Error("Open a chat first.");
                const result = await (name === "undo"
                    ? client.sessions.undo(id)
                    : name === "redo"
                      ? client.sessions.redo(id)
                      : name === "abort"
                        ? client.sessions.abort(id)
                        : client.sessions.summarize(id));
                show(name, result);
                return true;
            }
            case "queue":
                if (!id) throw new Error("Open a chat first.");
                show(
                    "Queue",
                    args === "clear"
                        ? await client.sessions.clearQueue(id)
                        : args === "pop"
                          ? await client.sessions.popQueue(id)
                          : await client.sessions.queue(id),
                );
                return true;
            case "permissions":
                if (!id) throw new Error("Open a chat first.");
                show(
                    "Permissions",
                    await client.interactions.permissions.list(id),
                );
                return true;
            case "questions":
                if (!id) throw new Error("Open a chat first.");
                show(
                    "Questions",
                    await client.interactions.questions.list(id),
                );
                return true;
            case "yolo":
            case "dangerously-skip-permissions":
            case "skip-permissions":
                if (!id) throw new Error("Open a chat first.");
                setSkipPermissions((x) => !x);
                return true;
            case "piss":
            case "cuss":
            case "glitch":
            case "disco":
            case "gangfight":
            case "praise":
                fxScope.current = { epoch: selectionEpoch.current, client, directory: directory, preferenceDirectory: prefs.directory };
                setEffectRevision((old) => old + 1);
                setEffect(name);
                return true;
            default:
                return runNative(name, args);
        }
    };
    const performSend = async (text: string, files: File[] = []) => {
        const generation = selectionEpoch.current;
        const isCurrent = () => currentClient.current === client && generation === selectionEpoch.current;
        const assertCurrent = () => {
            if (!isCurrent()) throw new Error("Chat changed. Submit again in the current chat.");
        };
        setError("");
        try {
            if (text.startsWith("/") && files.length) throw new Error("Attachments can only be sent with a chat message, not a slash command.");
            if (text.startsWith("/"))
                await executeCommand(text, catalog, {
                    local,
                    confirm: window.confirm.bind(window),
                    forward: async (command) => {
                        assertAccount(selection.current.model, selection.current.connectionId);
                        const session = await ensureSession();
                        assertCurrent();
                        const { model, agent, thinking, connectionId } = await prepare(tab);
                        assertAccount(model, connectionId);
                        const { name, args } = parseCommand(command, []);
                        const slash = model.indexOf("/");
                        const result = await client.sessions.command(session, name, {
                                arguments: args,
                                agent,
                                ...(slash > 0
                                    ? {
                                          model: {
                                              providerId: model.slice(0, slash),
                                              modelId: model.slice(slash + 1),
                                              variant: wireVariant(tab, thinking),
                                              connectionId: connectionId || undefined,
                                          },
                                      }
                                    : {}),
                            });
                        if (isCurrent()) showResult("Command result", result);
                    },
                });
            else await promptSession(text, undefined, files);
        } catch (e) {
            if (isCurrent()) notify(errorMessage(e));
            throw e;
        }
    };
    const sends = useRef(new Map<string, { text: string; files: File[]; promise: Promise<void> }[]>());
    const send = (text: string, files: File[] = []): Promise<void> => {
        const key = `${prefs.server}:${tabKey}`;
        const flights = sends.current.get(key) || [];
        const existing = flights.find(f => f.text === text && f.files.length === files.length && f.files.every((file, i) => file === files[i]));
        if (existing) return existing.promise;
        const flight = { text, files: [...files], promise: Promise.resolve() };
        flight.promise = performSend(text, files).finally(() => {
            const remaining = (sends.current.get(key) || []).filter(f => f !== flight);
            if (remaining.length) sends.current.set(key, remaining); else sends.current.delete(key);
        });
        sends.current.set(key, [...flights, flight]);
        return flight.promise;
    };
    const choose = (value: string) => {
        if (picker === "model") setModel(value);
        else if (picker === "agent") setAgent(value);
        else if (picker === "thinking") setThinking(value);
        else if (picker === "directory") void setDirectory(value).catch(e => notify(errorMessage(e)));
        else if (picker === "skill") insertSkill(value);
        else openSession(value);
        setPicker(undefined);
    };
    const groupedModels = useMemo(() => groupedModelChoices(catalogPending.scope === catalogScope ? models : [], recentModels, model), [models, recentModels, model, catalogPending.scope, catalogScope]);
    const pickerLoading = (picker === 'model' || picker === 'agent') && (catalogPending.scope !== catalogScope || catalogPending[picker]);
    const choices: Choice[] =
        picker === "directory" ? directories : picker === "skill" ? skills :
        picker === "model"
            ? groupedModels
            : picker === "agent"
              ? (catalogPending.scope === catalogScope ? agents : [])
              : picker === "thinking"
                ? ["", "minimal", "low", "medium", "high", "xhigh", ...(model.endsWith("/gpt-5.6") ? ["ultra"] : [])].map(
                      (v) => ({ id: v, label: v || "Provider default" }),
                  )
                : (picker === "subagents" ? children : sessions)
                      .map((s) => ({
                          id: s.id,
                          label: s.title || "Untitled chat",
                          description: s.directory,
                      }));
    useEffect(() => {
        let alive = true;
        setDirectories([]);
        setSkills([]);
        setChildren([]);
        const request = picker === "directory" && id
            ? client.operations.request("v2.sessions.directoryOptions", { path: { session_id: id }, query: { limit: 100, query: directoryQuery || undefined } }).then((paths) => {
                if (alive) setDirectories(paths.map((path) => ({ id: path, label: path })));
            })
            : picker === "skill"
              ? client.catalog.skills.list(directory).then((items) => {
                  if (alive) setSkills(items.map((s) => ({ id: s.name, label: s.name, description: s.description || undefined })));
              })
              : picker === "subagents" && id
                ? client.operations.request("v2.sessions.children", { path: { session_id: id } }).then((page) => {
                    if (alive) setChildren(page.items);
                })
                : undefined;
        void request?.catch((e) => { if (alive) notify(errorMessage(e)); });
        return () => { alive = false; };
    }, [picker, id, client, active?.directory, prefs.directory, directoryQuery, notify]);
    const usage = useMemo(() => messageUsage(chat.state.messages), [chat.state.messages]);
    return {
        tabs: tabState.tabs, tabKey, activateTab, closeTab,
        draft: tab.draft,
        files: tab.files || [],
        onFilesChange: (files: File[]) => { tab.files = files; persistTabs(); },
        onDraftChange: (text: string) => { tab.draft = text; persistTabs(); },
        onCycleAgent: () => { if (agents.length) setAgent(agents[(agents.findIndex(a => a.id === agent) + 1) % agents.length].id); },
        providerCatalog,
        activityPalette: resolveTheme(prefs.theme).colors,
        prefs,
        setPrefs,
        token,
        setToken,
        saveSettings,
        settings,
        setSettings,
        view,
        setView,
        id,
        setId,
        sessions: loadedListScope === listScope ? sessions : [],
        setSessions,
        cursor,
        search,
        setSearch,
        listBusy,
        loading: loadedListScope !== listScope || (listBusy && !sessions.length),
        error,
        setError,
        connected,
        sidebar,
        setSidebar,
        nav,
        setNav,
        hints,
        info,
        setInfo,
        models,
        model,
        agent,
        thinking,
        picker,
        setPicker,
        togglePicker,
        pickerLoading,
        skipPermissions,
        setSkipPermissions,
        effect,
        notify,
        client,
        chat,
        active,
        catalog,
        recentMore,
        pinSession,
        renameSession,
        deleteSession,
        openSession,
        newChat,
        perform,
        send,
        choices,
        usage,
        choose,
        setDirectoryQuery,
        directory,
        setDirectory,
        recentDirectories: [...new Set([prefs.directory, ...tabState.tabs.map(tabDirectory), ...sessions.map(s => s.directory)].filter(Boolean))],
        onSelectConnection,
        selectedConnection: connectionId && model ? { providerId: model.split("/")[0], connectionId } : undefined,
        effectRevision,
        draftInsertion,
        connectionId,
        setConnectionId,
        children,
        setModel,
        setAgent,
        setThinking,
    };
}
