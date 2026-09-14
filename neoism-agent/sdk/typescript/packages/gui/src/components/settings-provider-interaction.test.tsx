import { SkeletonRows } from "./Skeleton";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import type { NeoismClient } from "@neoism/sdk";
// Deterministic hook host, matching the controller tests: no browser/DOM dependency.
const host = vi.hoisted(() => {
    let slots: any[] = [],
        cursor = 0;
    let effects: (() => void)[] = [];
    let dirty = false;
    const equal = (a?: unknown[], b?: unknown[]) =>
        !!a &&
        !!b &&
        a.length === b.length &&
        a.every((v, i) => Object.is(v, b[i]));
    return {
        reset() {
            slots = [];
            cursor = 0;
            effects = [];
            dirty = false;
        },
        begin() {
            cursor = 0;
            dirty = false;
        },
        flush() {
            const pending = effects;
            effects = [];
            pending.forEach((f) => f());
            return dirty;
        },
        cleanup() {
            slots.forEach((s) => s?.cleanup?.());
        },
        replayEffects() {
            slots.forEach((s) => {
                if (s?.setup) {
                    s.cleanup?.();
                    s.cleanup = s.setup();
                }
            });
        },
        useState(initial: any) {
            const i = cursor++;
            if (!slots[i])
                slots[i] = {
                    value: typeof initial === "function" ? initial() : initial,
                    set: (next: any) => {
                        const value =
                            typeof next === "function"
                                ? next(slots[i].value)
                                : next;
                        if (!Object.is(value, slots[i].value)) {
                            slots[i].value = value;
                            dirty = true;
                        }
                    },
                };
            return [slots[i].value, slots[i].set];
        },
        useRef(initial: any) {
            const i = cursor++;
            return (slots[i] ||= { current: initial });
        },
        useMemo(fn: () => any, deps: any[]) {
            const i = cursor++;
            if (!slots[i] || !equal(slots[i].deps, deps))
                slots[i] = { value: fn(), deps };
            return slots[i].value;
        },
        useEffect(fn: () => any, deps: any[]) {
            const i = cursor++;
            if (slots[i] && equal(slots[i].deps, deps)) return;
            const old = slots[i];
            slots[i] = { deps, setup: fn };
            effects.push(() => {
                old?.cleanup?.();
                slots[i].cleanup = fn();
            });
        },
    };
});
vi.mock("react", () => ({ ...host, useLayoutEffect: host.useEffect }));

import {
    ProviderDirectory,
    ProviderRows,
    PROVIDER_WINDOW,
} from "./ProviderDirectory";
import { Settings, ThemePicker } from "./Settings";
import { ThemePreview } from "./ThemePreview";
import { themeOptions } from "../appearance";
import { ProviderConnections } from "./ProviderConnections";
import { defaultPreferences } from "../types";

function nodes(node: any): any[] {
    if (!node || typeof node !== "object") return [];
    if (Array.isArray(node)) return node.flatMap(nodes);
    return [node, ...nodes(node.props?.children)];
}
const find = (tree: any, predicate: (node: any) => boolean) =>
    nodes(tree).find(predicate);
const button = (tree: any, label: string) =>
    find(tree, (n) => n.type === "button" && nodesText(n).includes(label));
function nodesText(node: any): string {
    if (typeof node === "string") return node;
    if (Array.isArray(node)) return node.map(nodesText).join("");
    return nodesText(node?.props?.children ?? "");
}
let component: any, props: any, tree: any;
const root = { scrollTop: 0 };
const focus = vi.fn();
function render() {
    let again = true;
    let count = 0;
    while (again) {
        if (++count > 20) throw new Error("Render loop");
        host.begin();
        tree = component(props);
        for (const n of nodes(tree)) {
            if (!n.props?.ref || typeof n.props.ref !== "object") continue;
            n.props.ref.current = {
                showModal() {},
                close() {},
                focus,
                closest: () => root,
                querySelectorAll: () => [],
            };
        }
        again = host.flush();
    }
    return tree;
}
async function settle() {
    for (let i = 0; i < 8; i++) {
        await Promise.resolve();
        render();
    }
}
const catalog = {
    all: Array.from({ length: 85 }, (_, i) => ({
        id: `p${i}`,
        name: `Provider ${i}`,
        models: {},
    })),
    connected: ["p0"],
    default: {},
};
const noop = () => {};
function startDirectory(list = vi.fn(async (_directory: string) => catalog)) {
    const client = {
        catalog: { providers: { list } },
    } as unknown as NeoismClient;
    // Obtain the private scoped component via the public wrapper's element.
    component = ProviderDirectory;
    props = { client, directory: "/work", children: vi.fn(noop) };
    const element = render();
    host.cleanup();
    host.reset();
    component = element.type;
    render();
    return list;
}
beforeEach(() => {
    host.reset();
    root.scrollTop = 0;
    focus.mockClear();
    vi.stubGlobal("document", { activeElement: null });
    vi.stubGlobal("HTMLElement", class {});
});
afterEach(() => {
    host.cleanup();
    vi.unstubAllGlobals();
});

describe("provider directory lifecycle", () => {
    it("single-flights the catalog even when StrictMode replays effects", async () => {
        const list = startDirectory();
        host.replayEffects();
        await settle();
        expect(list).toHaveBeenCalledExactlyOnceWith("/work");
        expect(find(tree, (n) => n.type === ProviderRows)).toBeDefined();
    });
    it("autoloads once, reveals locally through the end, and searches outside the window", async () => {
        const list = startDirectory();
        await settle();
        expect(list).toHaveBeenCalledExactlyOnceWith("/work");
        const rows = () => find(tree, (n) => n.type === ProviderRows).props;
        expect(rows().limit).toBe(PROVIDER_WINDOW);
        for (let i = 0; i < 3; i++) {
            button(tree, "Load more").props.onClick();
            render();
        }
        expect(button(tree, "Load more")).toBeUndefined();
        expect(nodesText(tree)).toContain("All matching providers shown.");
        find(
            tree,
            (n) => n.props?.["aria-label"] === "Search providers",
        ).props.onChange({ target: { value: "Provider 84" } });
        render();
        expect(rows().query).toBe("Provider 84");
        expect(rows().limit).toBe(PROVIDER_WINDOW);
        expect(list).toHaveBeenCalledTimes(1);
    });
    it.each([false, true])(
        "keeps query/window and refreshes after returning from accounts=%s",
        async (accounts) => {
            const list = startDirectory();
            await settle();
            find(
                tree,
                (n) => n.props?.["aria-label"] === "Search providers",
            ).props.onChange({ target: { value: "Provider" } });
            render();
            button(tree, "Load more").props.onClick();
            render();
            root.scrollTop = 480;
            const rows = find(tree, (n) => n.type === ProviderRows).props;
            rows[accounts ? "manage" : "connect"]("p0");
            render();
            expect(tree.props.className).toBe("provider-flow");
            expect(props.children).toHaveBeenLastCalledWith("p0", accounts);
            root.scrollTop = 0;
            button(tree, "Providers").props.onClick();
            render();
            await settle();
            const returned = find(tree, (n) => n.type === ProviderRows).props;
            expect(returned.query).toBe("Provider");
            expect(returned.limit).toBe(PROVIDER_WINDOW * 2);
            expect(root.scrollTop).toBe(480);
            expect(list).toHaveBeenCalledTimes(2);
            expect(list.mock.calls.every((args) => args.length === 1)).toBe(
                true,
            );
        },
    );
    it("observes the host scroll ancestor, reveals and disconnects", async () => {
        let callback: any;
        const observe = vi.fn(),
            disconnect = vi.fn();
        let options: any;
        vi.stubGlobal(
            "IntersectionObserver",
            class {
                constructor(cb: any, opts: any) {
                    callback = cb;
                    options = opts;
                }
                observe = observe;
                disconnect = disconnect;
            },
        );
        const list = startDirectory();
        await settle();
        expect(options.root).toBe(root);
        expect(observe).toHaveBeenCalledTimes(1);
        callback([{ isIntersecting: true }]);
        render();
        expect(find(tree, (n) => n.type === ProviderRows).props.limit).toBe(48);
        expect(disconnect).toHaveBeenCalled();
        expect(list).toHaveBeenCalledTimes(1);
    });
    it("keys both directory and client scope changes, drops old data and ignores stale results", async () => {
        let resolve!: (value: any) => void;
        startDirectory(
            vi.fn(
                () =>
                    new Promise((r) => {
                        resolve = r;
                    }),
            ),
        );
        host.cleanup();
        host.reset();
        component = ProviderDirectory;
        render();
        const initialKey = tree.key;
        props = { ...props, directory: "/other" };
        render();
        expect(tree.key).not.toBe(initialKey);
        const directoryKey = tree.key;
        const list = vi.fn(async () => ({ ...catalog, all: [] }));
        props = { ...props, client: { catalog: { providers: { list } } } };
        render();
        expect(tree.key).not.toBe(directoryKey);
        const scoped = tree.type;
        host.cleanup();
        host.reset();
        component = scoped;
        render();
        expect(find(tree, (n) => n.type === SkeletonRows)?.props.kind).toBe("provider");
        expect(find(tree, (n) => n.type === ProviderRows)).toBeUndefined();
        resolve(catalog);
        await settle();
        expect(
            find(tree, (n) => n.type === ProviderRows).props.catalog.all,
        ).toEqual([]);
        expect(list).toHaveBeenCalledExactlyOnceWith("/other");
    });
});

describe("responsive settings navigation", () => {
    const labelled = (label: string) => find(tree, n => n.type === "button" && n.props["aria-label"] === label);
    const layout = () => find(tree, n => n.props?.className?.includes("settings-layout"));
    const start = (extra = {}) => {
        component = Settings;
        props = { client: {}, value: defaultPreferences, token: "secret", save: vi.fn(), close: vi.fn(), ...extra };
        render();
    };
    it("starts with mobile categories and desktop General, retains selection and drafts through Back", () => {
        start();
        expect(layout().props.className).toContain("settings-landing");
        expect(labelled("General").props["aria-current"]).toBe("page");
        for (const category of ["General", "Appearance", "Servers", "Providers"]) {
            labelled(category).props.onClick(); render();
            expect(layout().props.className).toContain("settings-detail");
            expect(find(tree, n => n.type === "h2" && nodesText(n) === category)).toBeDefined();
            expect(labelled(category).props["aria-current"]).toBe("page");
            expect(nodes(tree).filter(n => n.type === "dialog")).toHaveLength(1);
            if (category === "General") {
                find(tree, n => n.type === "input" && n.props.maxLength === 80).props.onChange({ target: { value: "New name" } });
                render();
            }
            focus.mockClear();
            labelled("Back to settings categories").props.onClick(); render();
            expect(layout().props.className).toContain("settings-landing");
            // The same detail stays mounted for desktop/resizing; only CSS changes visibility.
            expect(labelled(category).props["aria-current"]).toBe("page");
            expect(focus).toHaveBeenCalledTimes(1);
            expect(props.save).not.toHaveBeenCalled();
            expect(props.close).not.toHaveBeenCalled();
        }
        labelled("General").props.onClick(); render();
        expect(find(tree, n => n.type === "input" && n.props.maxLength === 80).props.value).toBe("New name");
        find(tree, n => n.type === "form").props.onSubmit({ preventDefault: noop });
        expect(props.save).toHaveBeenCalledWith({ ...defaultPreferences, name: "New name" }, "secret");
    });
    it("uses the top-left back control for themes without exiting the category or saving", () => {
        start();
        labelled("Appearance").props.onClick(); render();
        button(tree, "Theme").props.onClick(); render();
        expect(find(tree, n => n.type === ThemePicker).props.showBack).toBe(false);
        labelled("Back to Appearance").props.onClick(); render();
        expect(find(tree, n => n.type === ThemePicker)).toBeUndefined();
        expect(layout().props.className).toContain("settings-detail");
        expect(props.close).not.toHaveBeenCalled();
        expect(props.save).not.toHaveBeenCalled();
    });
    it("preserves direct provider entry, selection context and inline auth ownership", () => {
        const onSelectConnection = vi.fn();
        const selectedConnection = { providerId: "openai", connectionId: "work" };
        start({ initialProviderId: "openai", workspaceId: "workspace", selectedConnection, onSelectConnection });
        const provider = () => find(tree, n => n.type === ProviderConnections);
        expect(tree.props.className).toContain("settings-connecting");
        expect(provider().props).toMatchObject({ initialProviderId: "openai", workspaceId: "workspace", selectedConnection, onSelectConnection });
        expect(find(tree, n => n.type === "form")).toBeUndefined();
        expect(labelled("Back to settings categories")).toBeUndefined();
        provider().props.onFlowChange(false); render();
        labelled("Back to settings categories").props.onClick(); render();
        labelled("Providers").props.onClick(); render();
        expect(provider().props.initialProviderId).toBeUndefined();
        expect(tree.props.className).not.toContain("settings-connecting");
        labelled("Close settings").props.onClick();
        expect(props.close).toHaveBeenCalledTimes(1);
    });
});

describe("theme preview candidates", () => {
    const preview = () => find(tree, n => n.type === ThemePreview)?.props.theme;
    const searchInput = () => find(tree, n => n.props?.["aria-label"] === "Search themes");
    const results = () => find(tree, n => n.props?.className === "settings-theme-results");
    const start = () => {
        component = ThemePicker;
        props = { selected: "pastelbeans", query: "", search: vi.fn((query: string) => { props.query = query; }), choose: vi.fn(), back: vi.fn() };
        render();
    };
    const key = (target: any, key: string, extra = {}) => {
        const event = { key, nativeEvent: { isComposing: false }, preventDefault: vi.fn(), ...extra };
        target.props.onKeyDown(event); render();
        return event;
    };
    it("previews hover and focus independently of the selected draft; clicks apply only the clicked candidate", () => {
        start();
        expect(preview().id).toBe("pastelbeans");
        button(tree, "Github Light").props.onMouseEnter(); render();
        expect(preview().id).toBe("github_light");
        expect(button(tree, "Github Light").props["data-preview"]).toBe(true);
        expect(button(tree, "Github Light").props["aria-pressed"]).toBe(false);
        expect(button(tree, "Pastelbeans").props["aria-pressed"]).toBe(true);
        button(tree, "Github Dark").props.onFocus(); render();
        expect(preview().id).toBe("github_dark");
        expect(props.choose).not.toHaveBeenCalled();
        button(tree, "Github Dark").props.onClick();
        expect(props.choose).toHaveBeenCalledExactlyOnceWith("github_dark");
    });
    it("arrows from search preview without applying; Enter chooses the candidate", () => {
        start();
        const index = themeOptions.findIndex(t => t.id === props.selected);
        expect(key(searchInput(), "ArrowDown").preventDefault).toHaveBeenCalled();
        expect(preview().id).toBe(themeOptions[index + 1].id);
        key(searchInput(), "ArrowUp");
        expect(preview().id).toBe("pastelbeans");
        key(searchInput(), "ArrowDown");
        expect(props.choose).not.toHaveBeenCalled();
        key(searchInput(), "Enter");
        expect(props.choose).toHaveBeenCalledExactlyOnceWith(themeOptions[index + 1].id);
    });
    it("wraps arrow navigation, focuses and scrolls list rows, and ignores composition/modifier keys", () => {
        start();
        button(tree, themeOptions[0].name).props.onFocus(); render();
        const scrollIntoView = vi.fn(), rowFocus = vi.fn();
        results().props.ref.current = { querySelectorAll: () => themeOptions.map(() => ({ focus: rowFocus, scrollIntoView })) };
        key(results(), "ArrowUp");
        expect(preview().id).toBe(themeOptions.at(-1)!.id);
        expect(rowFocus).toHaveBeenCalledWith({ preventScroll: true });
        expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest", inline: "nearest" });
        key(results(), "ArrowDown");
        expect(preview().id).toBe(themeOptions[0].id);
        expect(key(searchInput(), "ArrowDown", { altKey: true }).preventDefault).not.toHaveBeenCalled();
        expect(preview().id).toBe(themeOptions[0].id);
        key(searchInput(), "Enter", { nativeEvent: { isComposing: true } });
        expect(props.choose).not.toHaveBeenCalled();
        key(results(), "Enter");
        expect(props.choose).toHaveBeenCalledExactlyOnceWith(themeOptions[0].id);
    });
    it("filters the candidate and Enter target together, with no stale preview for zero matches", () => {
        start();
        const filter = (query: string) => { searchInput().props.onChange({ target: { value: query } }); render(); };
        filter("github_light");
        expect(preview().id).toBe("github_light");
        filter("no-such-theme");
        expect(preview()).toBeUndefined();
        expect(nodesText(tree)).toContain("No matching themes.");
        key(searchInput(), "ArrowDown"); key(searchInput(), "Enter");
        expect(props.choose).not.toHaveBeenCalled();
        filter("");
        expect(preview().id).toBe(themeOptions[0].id);
        expect(nodes(tree).filter(n => n.type === "button" && n.props["aria-pressed"] !== undefined)).toHaveLength(101);
        button(tree, "Back to Appearance").props.onClick();
        expect(props.back).toHaveBeenCalledTimes(1);
        expect(props.choose).not.toHaveBeenCalled();
    });
});

describe("theme draft navigation", () => {
    it("saves the code font independently from the interface font", () => {
        const save = vi.fn();
        component = Settings;
        props = {client:{},value:defaultPreferences,token:"",save,close:vi.fn()};
        render();
        button(tree, "Appearance").props.onClick(); render();
        const codeFont = find(tree, node => node.props?.["aria-label"] === "Code font");
        expect(codeFont.props.value).toBe("jetbrains-mono");
        codeFont.props.onChange({target:{value:"geist-mono"}}); render();
        find(tree, node => node.type === "form").props.onSubmit({preventDefault:noop});
        expect(save).toHaveBeenCalledWith({...defaultPreferences,font:"geist",codeFont:"geist-mono"}, "");
    });
    it("selects into the draft, restores focus on Back/Escape, saves only on submit", () => {
        const save = vi.fn(),
            close = vi.fn();
        component = Settings;
        props = {
            client: {},
            value: defaultPreferences,
            token: "secret",
            save,
            close,
        };
        render();
        button(tree, "Appearance").props.onClick();
        render();
        focus.mockClear();
        const open = () => {
            button(tree, "Theme").props.onClick();
            render();
        };
        open();
        expect(find(tree, (n) => n.type === ThemePicker)).toBeDefined();
        find(tree, (n) => n.type === ThemePicker).props.choose("chadracula");
        render();
        expect(nodesText(button(tree, "Theme"))).toContain("Chadracula");
        expect(focus).toHaveBeenCalled();
        expect(save).not.toHaveBeenCalled();
        open();
        find(tree, (n) => n.type === ThemePicker).props.back();
        render();
        expect(find(tree, (n) => n.type === ThemePicker)).toBeUndefined();
        open();
        tree.props.onKeyDown({
            key: "Escape",
            preventDefault: noop,
            stopPropagation: noop,
        });
        render();
        expect(close).not.toHaveBeenCalled();
        expect(focus).toHaveBeenCalledTimes(3);
        find(tree, (n) => n.type === "form").props.onSubmit({
            preventDefault: noop,
        });
        expect(save).toHaveBeenCalledWith(
            { ...defaultPreferences, theme: "chadracula" },
            "secret",
        );
        expect(close).toHaveBeenCalledTimes(1);
    });
    it("Cancel discards the selected draft without saving", () => {
        const save = vi.fn(),
            close = vi.fn();
        component = Settings;
        props = {
            client: {},
            value: defaultPreferences,
            token: "",
            save,
            close,
        };
        render();
        button(tree, "Appearance").props.onClick();
        render();
        button(tree, "Theme").props.onClick();
        render();
        find(tree, (n) => n.type === ThemePicker).props.choose("chadracula");
        render();
        button(tree, "Cancel").props.onClick();
        expect(close).toHaveBeenCalledTimes(1);
        expect(save).not.toHaveBeenCalled();
    });
});
