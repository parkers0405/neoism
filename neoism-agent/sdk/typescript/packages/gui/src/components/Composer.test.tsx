import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactElement } from "react";
const host = vi.hoisted(() => {
    let slots: any[] = [], cursor = 0;
    let effects: (() => void)[] = [];
    let dirty = false;
    const equal = (a?: unknown[], b?: unknown[]) => !!a && !!b && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
    return {
        
        
        reset() { slots = []; cursor = 0; effects = []; dirty = false; },
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
vi.mock("react", async (original) => ({ ...await original<typeof import("react")>(), ...host, useLayoutEffect: host.useEffect, useId: () => host.useRef("composer-test").current }));
import { ProjectPicker } from "./ProjectPicker";
import { Composer, ComposerFooter, type ComposerProps } from "./Composer";
import { readFileSync } from "node:fs";
let props: ComposerProps;
let tree: ReactElement;
function render() { let again = true; let count = 0; while (again) { if (++count > 20) throw new Error("Render loop"); host.begin(); tree = Composer(props); again = host.flush(); } return tree; }
function nodes(node: any = tree): any[] { if (!node || typeof node !== "object") return []; if (Array.isArray(node)) return node.flatMap(n => nodes(n)); return [node, ...nodes(node.props?.children ?? null)]; }
function find(predicate: (n: any) => boolean) { const result = nodes().find(predicate); if (!result) throw new Error("Missing element"); return result; }
const textarea = () => find(n => n.type === "textarea");
const button = (label: string) => find(n => n.type === "button" && n.props["aria-label"] === label);
const file = (name: string, size = 1024) => ({ name, size, lastModified: 1, type: "text/plain" }) as File;
function key(key: string, shiftKey = false, isComposing = false) { const e = { key, shiftKey, nativeEvent: { isComposing }, preventDefault: vi.fn() }; textarea().props.onKeyDown(e); render(); return e; }
function change(value: string) { textarea().props.onChange({ target: { value } }); render(); }
beforeEach(() => { host.reset(); props = { busy: false, commands: [], send: vi.fn(async () => {}), abort: vi.fn(), model: "model", agent: "Build", thinking: "High", openPicker: vi.fn() }; render(); });
describe("native composer", () => {
    it("places the input band inside the raised surface and chips in its sibling skirt", () => {
        const surface = find(n => n.props?.className === "composer native-composer-surface");
        const skirt = find(n => n.props?.className === "native-composer-skirt");
        expect(nodes(surface).some(n => n.props?.className === "native-composer-input-band")).toBe(true);
        expect(nodes(surface).some(n => n.props?.className === "native-composer-chips")).toBe(false);
        expect(nodes(skirt).some(n => n.props?.className === "native-composer-chips")).toBe(true);
        const css = readFileSync(new URL("./composer-native.css", import.meta.url), "utf8");
        expect(css).toContain("top: calc((100% - 26px) / 2)");
        expect(css).toContain("border-radius: 18px; box-shadow: none");
        expect(css).toContain("max-height: 110px");
        expect(css).toContain("font-size: 16px");
    });
    it("uses a real multiple file picker, shows size, removes files and submits attachments", async () => {
        const input = find(n => n.type === "input" && n.props.type === "file");
        expect(input.props.multiple).toBe(true);
        const click = vi.fn(); input.props.ref.current = { click };
        button("Add attachments").props.onClick(); expect(click).toHaveBeenCalledOnce();
        const a = file("one.txt"), b = file("two.txt");
        const target = { files: [a, b], value: "fakepath" }; input.props.onChange({ target }); render();
        expect(target.value).toBe("");
        expect(find(n => n.type === "li").props.title).toContain("1.0 KB");
        button("Remove one.txt").props.onClick(); render();
        expect(button("Send message").props.disabled).toBe(false);
        button("Send message").props.onClick(); await Promise.resolve(); render();
        expect(props.send).toHaveBeenCalledWith("", [b]);
        expect(nodes().some(n => n.props?.["aria-label"] === "Selected attachments")).toBe(false);
    });
    it("preserves drafts and files on upload failure with visible error", async () => {
        props.send = vi.fn(async () => { throw new Error("Upload too large (limit 10 MB)"); }); render();
        change("keep me");
        const a = file("large.txt"); find(n => n.type === "input").props.onChange({ target: { files: [a], value: "" } }); render();
        button("Send message").props.onClick(); await Promise.resolve(); render();
        expect(textarea().props.value).toBe("keep me");
        expect(button("Remove large.txt")).toBeTruthy();
        expect(find(n => n.props?.role === "alert").props.children).toContain("limit 10 MB");
    });
    it("single-flights sends and does not erase edits or files added during a send", async () => {
        let resolve!: () => void; props.send = vi.fn(() => new Promise<void>(r => { resolve = r; })); render();
        change("first"); button("Send message").props.onClick(); button("Send message").props.onClick();
        expect(props.send).toHaveBeenCalledTimes(1);
        change("new draft"); const a = file("new.txt"); find(n => n.type === "input").props.onChange({ target: { files: [a], value: "" } }); render();
        resolve(); await Promise.resolve(); render(); expect(textarea().props.value).toBe("new draft"); expect(button("Remove new.txt")).toBeTruthy();
    });
    it("scopes local drafts, attachments and pending sends by tab key", async () => {
        let resolve!: () => void; props = { ...props, tabKey: "a", send: vi.fn(() => new Promise<void>(r => { resolve = r; })) }; render();
        change("old"); button("Send message").props.onClick();
        props = { ...props, tabKey: "b" }; render(); change("new");
        find(n => n.type === "input").props.onChange({ target: { files: [file("b.txt")], value: "" } }); render();
        expect(button("Send message").props.disabled).toBe(false);
        resolve(); await Promise.resolve(); render(); expect(textarea().props.value).toBe("new"); expect(button("Remove b.txt")).toBeTruthy();
        props = { ...props, tabKey: "a" }; render(); expect(textarea().props.value).toBe("");
        props = { ...props, tabKey: "b" }; render(); expect(textarea().props.value).toBe("new"); expect(button("Remove b.txt")).toBeTruthy();
    });
    it("never calls a new tab's controlled callbacks from an old submit", async () => {
        let resolve!: () => void;
        props = { ...props, tabKey: "a", draft: "old", files: [file("a")], onDraftChange: vi.fn(), onFilesChange: vi.fn(), send: vi.fn(() => new Promise<void>(r => { resolve = r; })) }; render();
        button("Send message").props.onClick();
        props = { ...props, tabKey: "b", draft: "new", files: [file("b")], onDraftChange: vi.fn(), onFilesChange: vi.fn() }; render();
        resolve(); await Promise.resolve(); render();
        expect(props.onDraftChange).not.toHaveBeenCalled(); expect(props.onFilesChange).not.toHaveBeenCalled(); expect(textarea().props.value).toBe("new");
    });
    it("does not publish stale setters or focus after a keyed unmount", async () => {
        let resolve!: () => void;
        props = { ...props, tabKey: "a", draft: "old", files: [file("a")], onDraftChange: vi.fn(), onFilesChange: vi.fn(), send: vi.fn(() => new Promise<void>(r => { resolve = r; })) }; render();
        const focus = vi.fn(); textarea().props.ref.current = { focus };
        button("Send message").props.onClick(); host.cleanup();
        resolve(); await Promise.resolve();
        expect(props.onDraftChange).not.toHaveBeenCalled(); expect(props.onFilesChange).not.toHaveBeenCalled(); expect(focus).not.toHaveBeenCalled();
    });
    it("preserves IME and Shift Enter, cycles agents on Tab and leaves Shift Tab accessible", () => {
        props.onCycleAgent = vi.fn(); render(); change("hello");
        expect(key("Enter", false, true).preventDefault).not.toHaveBeenCalled();
        expect(key("Enter", true).preventDefault).not.toHaveBeenCalled(); expect(props.send).not.toHaveBeenCalled();
        expect(key("Tab").preventDefault).toHaveBeenCalled(); expect(props.onCycleAgent).toHaveBeenCalledOnce();
        expect(key("Tab", true).preventDefault).not.toHaveBeenCalled();
    });
    it("navigates slash commands in both directions and retains Shift Enter", () => {
        props.commands = [{ name: "alpha", description: "Alpha", aliases: [] }, { name: "beta", description: "Beta", aliases: [] }]; render(); change("/");
        key("Tab"); expect(textarea().props["aria-activedescendant"]).toBe("composer-test-1");
        key("Tab", true); expect(textarea().props["aria-activedescendant"]).toBe("composer-test-0");
        expect(key("Enter", true).preventDefault).not.toHaveBeenCalled(); key("Enter"); expect(props.send).toHaveBeenCalledWith("/alpha", undefined);
        key("Escape"); expect(textarea().props["aria-expanded"]).toBe(false);
    });
    it("can delegate the footer to an uncapped host without duplicating it", () => {
        expect(nodes().filter(n => n.type === ComposerFooter)).toHaveLength(1);
        props.showFooter = false; render();
        expect(nodes().some(n => n.type === ComposerFooter)).toBe(false);
        expect(textarea()).toBeTruthy();
    });
    it("renders directory picker, optional hints and abort", () => {
        props = { ...props, directory: "/work", busy: true, showHints: false }; render();
        const onDirectoryChange = vi.fn();
        props = { ...props, onDirectoryChange, recentDirectories: ["/recent"] }; render();
        const footer = find(n => n.type === ComposerFooter);
        const picker = nodes(ComposerFooter(footer.props)).find(n => n.type === ProjectPicker)!;
        expect(picker.props.directory).toBe("/work");
        expect(picker.props.onDirectoryChange).toBe(onDirectoryChange);
        expect(picker.props.recentDirectories).toEqual(["/recent"]);
        expect(nodes(ComposerFooter(footer.props)).some(n => n.props?.className === "native-composer-hints")).toBe(false);
        expect(nodes(ComposerFooter({ ...footer.props, showHints: true })).some(n => n.props?.className === "native-composer-hints")).toBe(true);
        button("Stop response").props.onClick(); expect(props.abort).toHaveBeenCalledOnce();
    });
});
