// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { NeoismClient, WorkflowDefinition } from "@neoism/sdk";
import { WorkflowDefinitionEditor } from "./components/WorkflowDefinitionEditor";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
const definition: WorkflowDefinition = { id: "test", name: "Test", prompt: "Do work", active: false, schedule: { frequency: "daily", interval: 1, timezone: "UTC", time: "09:00" } };
let root: Root | undefined;
let host: HTMLDivElement;
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; host?.remove(); });
async function mount(element: React.ReactNode) { host = document.createElement("div"); document.body.append(host); root = createRoot(host); await act(async () => root!.render(element)); }
function select(label: string) { const element = [...host.querySelectorAll("label")].find(e => e.textContent?.startsWith(label)); return element?.querySelector("select") ?? host.querySelector<HTMLSelectElement>(`select[id="${element?.htmlFor}"]`)!; }
async function choose(label: string, value: string) { const el = select(label); expect(el).toBeTruthy(); await act(async () => { el.value = value; el.dispatchEvent(new Event("change", { bubbles: true })); }); }

describe("friendly workflow definition form", () => {
    it("SSR renders graphical options, native defaults and no shell or JSON editor", () => {
        const html = renderToStaticMarkup(<WorkflowDefinitionEditor value={definition} onChange={() => {}} />);
        expect(html).toContain("Enable schedule"); expect(html).toContain("Execution directory (optional)"); expect(html).toContain("Retryable error codes");
        expect(html).not.toContain("Description"); expect(html).not.toContain("Cancel"); expect(html).not.toContain("Create workflow"); expect(html).not.toContain("1000"); expect(html).not.toContain("60000");
        expect((html.match(/<textarea/g) ?? []).length).toBe(1);
    });
    it("reports errors and blocks native form submission even without onError", async () => {
        const error = vi.fn();
        await mount(<form><WorkflowDefinitionEditor value={{ ...definition, permissions: { bash: "ask" } }} onChange={() => {}} onError={error} /></form>);
        expect(error.mock.lastCall?.[0]).toContain("cannot ask");
        expect(host.querySelector("form")!.checkValidity()).toBe(false);
        await act(async () => root!.render(<form><WorkflowDefinitionEditor value={definition} onChange={() => {}} onError={error} /></form>));
        expect(error).toHaveBeenLastCalledWith(""); expect(host.querySelector("form")!.checkValidity()).toBe(true);
    });
    it("locks existing IDs and respects both own and ancestor disabled fieldsets", async () => {
        const change = vi.fn();
        await mount(<fieldset disabled><WorkflowDefinitionEditor value={definition} onChange={change} editing /></fieldset>);
        for (const el of host.querySelectorAll("input, textarea, select, button")) expect(el.hasAttribute("disabled") || !!el.closest("fieldset[disabled]")).toBe(true);
        expect(host.querySelector<HTMLInputElement>("input")!.disabled).toBe(true);
        await choose("Frequency", "hourly"); // Even a synthetic event cannot bypass the parent read-only fieldset.
        expect(change).not.toHaveBeenCalled();
        await act(async () => root!.render(<WorkflowDefinitionEditor value={definition} onChange={change} disabled />));
        for (const el of host.querySelectorAll("input, textarea, select, button")) expect(el.hasAttribute("disabled") || !!el.closest("fieldset[disabled]")).toBe(true);
    });
    it("normalizes concurrency when selecting forbid/replace", async () => {
        const change = vi.fn();
        await mount(<WorkflowDefinitionEditor value={{ ...definition, concurrency: { mode: "allow", maxRunning: 7 } }} onChange={change} />);
        await choose("When a run is still active", "replace"); expect(change.mock.lastCall?.[0].concurrency).toEqual({ mode: "replace", maxRunning: 1 });
        await choose("When a run is still active", "forbid"); expect(change.mock.lastCall?.[0].concurrency).toEqual({ mode: "forbid", maxRunning: 1 });
    });
    it("preserves unknown model/account/variant and never mutates on mount", async () => {
        const change = vi.fn(); const model = { providerId: "unlisted", id: "unlisted-model", connectionId: "old", variant: "custom" };
        await mount(<WorkflowDefinitionEditor value={{ ...definition, model }} onChange={change} />);
        expect(host.textContent).toContain("unlisted (current"); expect(host.textContent).toContain("unlisted-model (current"); expect(change).not.toHaveBeenCalled();
        await choose("Frequency", "hourly"); expect(change.mock.lastCall?.[0].model).toEqual(model);
    });
    it("renders existing ask patterns with an explicit remove action, not a valid new choice", async () => {
        const change = vi.fn(); const permission = { default: "deny", allow: ["git diff *"], deny: ["rm *"], ask: ["git push *"] };
        await mount(<WorkflowDefinitionEditor value={{ ...definition, permissions: { bash: permission } }} onChange={change} />);
        expect(host.textContent).toContain("git push *");
        expect([...host.querySelectorAll("option")].some(o => o.value === "ask")).toBe(false);
        const remove = [...host.querySelectorAll("button")].find(b => b.textContent === "Remove ask rules")!;
        await act(async () => remove.click());
        expect(change.mock.lastCall?.[0].permissions.bash).toEqual({ default: "deny", allow: ["git diff *"], deny: ["rm *"] });
        expect(permission.ask).toEqual(["git push *"]);
    });
    it("previews only through the SDK, in storage root, displaying exact server times", async () => {
        const preview = vi.fn().mockResolvedValue({ definition, sourcePath: "saved", upcoming: [{ local: "server-local 09:00 +05:30", scheduledAt: 123 }] });
        const use = vi.fn().mockResolvedValue({ preview });
        const client = { plugins: { use }, catalog: { agents: { list: vi.fn().mockResolvedValue([]) }, skills: { list: vi.fn().mockResolvedValue([]) }, providers: { configured: vi.fn().mockResolvedValue({ providers: [] }) } } } as unknown as NeoismClient;
        await mount(<WorkflowDefinitionEditor value={{ ...definition, directory: "/execution" }} onChange={() => {}} client={client} directory="/storage" editing />);
        expect(preview).not.toHaveBeenCalled();
        await act(async () => [...host.querySelectorAll("button")].find(b => b.textContent === "Preview saved schedule")!.click());
        expect(preview).toHaveBeenCalledWith("test", { directory: "/storage", scope: "workspace" }); expect(use.mock.lastCall?.[1]).toEqual({ directory: "/storage", scope: "workspace" });
        expect(host.textContent).toContain("server-local 09:00 +05:30"); expect(host.textContent).toContain("not unsaved form changes");
    });
    it("uses global preview and accounts even with a separate execution directory", async () => {
        const preview = vi.fn().mockResolvedValue({ definition, sourcePath: "saved", upcoming: [] });
        const use = vi.fn().mockResolvedValue({ preview });
        const connections = vi.fn().mockResolvedValue([]), workspaces = vi.fn();
        const request = vi.fn(async (operation: string) => operation === "v2.providers.configured" ? { providers: [] } : []);
        const client = { plugins: { use }, operations: { request }, management: { workspaces: { list: workspaces } }, catalog: { providers: { connections } } } as unknown as NeoismClient;
        await mount(<WorkflowDefinitionEditor value={{ ...definition, directory: "/execution", model: { providerId: "provider", id: "model" } }} onChange={() => {}} client={client} editing />);
        await act(async () => [...host.querySelectorAll("button")].find(b => b.textContent === "Preview saved schedule")!.click());
        expect(preview).toHaveBeenCalledWith("test", { scope: "installation" });
        expect(use.mock.lastCall?.[1]).toEqual({ scope: "installation" });
        expect(workspaces).not.toHaveBeenCalled();
        expect(connections).toHaveBeenCalledWith("provider", undefined);
        expect(host.textContent).toContain("The definition stays global.");
    });
    it("resolves account workspace IDs from the storage root, even when read-only", async () => {
        const connections = vi.fn().mockResolvedValue([{ connectionId: "account", label: "Team account", isDefault: true }]);
        const change = vi.fn();
        const client = { management: { workspaces: { list: vi.fn().mockResolvedValue([{ id: "workspace-id", root: "/storage" }]) } }, catalog: { agents: { list: vi.fn().mockResolvedValue([]) }, skills: { list: vi.fn().mockResolvedValue([]) }, providers: { configured: vi.fn().mockResolvedValue({ providers: [] }), connections } } } as unknown as NeoismClient;
        await mount(<WorkflowDefinitionEditor value={{ ...definition, directory: "/execution", model: { providerId: "provider", id: "model", connectionId: "unknown", variant: "old" } }} directory="/storage" client={client} onChange={change} disabled />);
        expect(connections).toHaveBeenCalledWith("provider", "workspace-id");
        expect(host.textContent).toContain("Team account");
        expect(change).not.toHaveBeenCalled();
    });
    it("discards an old workspace's late catalog response", async () => {
        let old!: (value: unknown[]) => void;
        const agents = vi.fn((directory: string) => directory === "/old" ? new Promise(r => { old = r; }) : Promise.resolve([{ name: "new-agent", hidden: false, mode: "primary" }]));
        const client = { catalog: { agents: { list: agents }, skills: { list: vi.fn().mockResolvedValue([]) }, providers: { configured: vi.fn().mockResolvedValue({ providers: [] }) } } } as unknown as NeoismClient;
        await mount(<WorkflowDefinitionEditor value={definition} onChange={() => {}} client={client} directory="/old" />);
        await act(async () => root!.render(<WorkflowDefinitionEditor value={definition} onChange={() => {}} client={client} directory="/new" />));
        expect(host.textContent).toContain("new-agent");
        await act(async () => old([{ name: "stale-agent", hidden: false, mode: "primary" }]));
        expect(host.textContent).not.toContain("stale-agent"); expect(host.textContent).toContain("new-agent");
    });
});
