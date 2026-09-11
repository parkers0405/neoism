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
