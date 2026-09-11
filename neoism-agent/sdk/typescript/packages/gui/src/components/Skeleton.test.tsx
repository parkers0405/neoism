// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { NeoismClient } from "@neoism/sdk";
import { readFileSync } from "node:fs";
import { ChoiceSkeleton, ConversationSkeleton, SkeletonRows } from "./Skeleton";
import { Navigation } from "./Navigation";
import { Library } from "./Library";
import { ProviderDirectory } from "./ProviderDirectory";
import { SidebarSubagents } from "./ChatDetails";
import { emptySubagents } from "./subagentController";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root | undefined;
let container: HTMLDivElement;
async function render(node: ReactNode) {
    if (!root) { container = document.createElement("div"); document.body.append(container); root = createRoot(container); }
    await act(async () => root!.render(node));
}
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; container?.remove(); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }

describe("contextual skeletons", () => {
    it("replaces recents only for initial/scope loading, and appends pagination placeholders", () => {
        const app = { prefs: { name: "You" }, sessions: [{ id: "one", title: "Valid chat" }], search: "", loading: true, listBusy: false } as unknown as Parameters<typeof Navigation>[0]["app"];
        const initial = renderToStaticMarkup(<Navigation app={app} />);
        expect(initial).not.toContain("Valid chat");
        expect(initial.match(/class="skeleton-row"/g)).toHaveLength(6);
        const more = renderToStaticMarkup(<Navigation app={{ ...app, loading: false, listBusy: true }} />);
        expect(more).toContain("Valid chat");
        expect(more).toContain("Loading more chats");
        expect(more.indexOf("Valid chat")).toBeLessThan(more.indexOf("Loading more chats"));
    });
    it("renders stable decorative shapes and only one accessible label", () => {
        for (const kind of ["session", "provider", "folder", "setting"] as const) {
            const html = renderToStaticMarkup(<SkeletonRows kind={kind} count={6} header />);
            expect(html).toContain('aria-busy="true"');
            expect(html).toContain('aria-hidden="true"');
            expect(html.match(/class="skeleton-label"/g)).toHaveLength(1);
            expect(html.match(/class="skeleton-row"/g)).toHaveLength(6);
            expect(html).not.toContain("<button");
            expect(html).toBe(renderToStaticMarkup(<SkeletonRows kind={kind} count={6} header />));
        }
        expect(renderToStaticMarkup(<ConversationSkeleton />)).toContain("skeleton-user");
        expect(renderToStaticMarkup(<ConversationSkeleton code={false} />)).not.toContain('class="skeleton-code"');
        expect(renderToStaticMarkup(<ChoiceSkeleton />)).toContain("Loading choices");
    });
    it("uses native palette, pauses offscreen, and preserves static reduced-motion shapes", () => {
        const css = readFileSync("src/components/skeleton.css", "utf8");
        expect(css).toContain("var(--theme-fg");
        expect(css).not.toMatch(/#[0-9a-f]{3,8}\b/i);
        expect(css).toContain("prefers-reduced-motion: reduce");
        expect(css).toContain("animation: none; opacity: .75");
        expect(css).toContain("animation-play-state: paused");
    });
    it("does not create a subagents section merely because requests are pending", () => {
        expect(renderToStaticMarkup(<SidebarSubagents data={emptySubagents()} open={() => {}} />)).toBe("");
    });
    it("keeps library initial and replacement scopes pending until their own requests resolve", async () => {
        const first = deferred<never[]>(), second = deferred<never[]>();
        const list = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
        const client = { operations: { request: async () => [{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: true }] }, management: { skills: { list } } } as unknown as NeoismClient;
        const replacement = { ...client };
        const view = (directory: string, c = client) => <Library kind="skills" client={c} directory={directory} openSession={() => {}} />;
        await render(view("/first"));
        expect(container.querySelector(".skeleton")).not.toBeNull();
        expect(container.textContent).not.toContain("Nothing here yet");
        await render(view("/second", replacement));
        await act(async () => first.resolve([]));
        expect(container.querySelector(".skeleton")).not.toBeNull();
        await act(async () => second.resolve([]));
        expect(container.querySelector(".skeleton")).toBeNull();
        expect(container.textContent).toContain("Nothing here yet");
    });
    it("clears providers on directory or client replacement and ignores late old results", async () => {
        const first = deferred<any>(), second = deferred<any>(), third = deferred<any>();
        const list = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
        const client = { catalog: { providers: { list } } } as unknown as NeoismClient;
        const other = { catalog: { providers: { list: () => third.promise } } } as unknown as NeoismClient;
        const view = (directory: string, c = client) => <ProviderDirectory client={c} directory={directory}>{() => null}</ProviderDirectory>;
        await render(view("/first"));
        expect(container.querySelector(".skeleton-provider")).not.toBeNull();
        await render(view("/second"));
        await act(async () => first.resolve({ all: [{ id: "old", name: "Obsolete provider" }], connected: [] }));
        expect(container.textContent).not.toContain("Obsolete provider");
        await act(async () => second.resolve({ all: [{ id: "current", name: "Current provider" }], connected: [] }));
        expect(container.textContent).toContain("Current provider");
        await render(view("/second", other));
        expect(container.textContent).not.toContain("Current provider");
        expect(container.querySelector(".skeleton-provider")).not.toBeNull();
        await act(async () => third.resolve({ all: [], connected: [] }));
        expect(container.querySelector(".skeleton-provider")).toBeNull();
    });
});
