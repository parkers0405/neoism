// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHttpClient, type NeoismClient } from "@neoism/sdk";
import { FolderPicker } from "./components/FolderPicker";
import { ProjectPicker } from "./components/ProjectPicker";
import { availableProjects, listServerFolders, projectName, rememberProject, savedProjects, selectServerProject } from "./projectPicker";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement, root: Root;
const listing = (path: string) => ({ path, parent: path === "/root" ? null : "/root", entries: path === "/root" ? [{name: "child", path: "/root/child"}] : [] });
function fakeClient(request = vi.fn(async ({query}: { query?: {path?: string} }) => listing(query?.path || "/root"))) {
    return { transport: { request }, management: {workspaces: {list: vi.fn(async () => [{root: "/root"}])}}, operations: {request: vi.fn(async (operation: string, input: {query?: {path?: string}}) => operation === "v2.directories.list" ? request(input) : ["/fallback"])} } as unknown as NeoismClient;
}
async function click(element: Element | null, type = "click") { expect(element).not.toBeNull(); await act(async () => { element!.dispatchEvent(new MouseEvent(type, { bubbles: true })); }); }
function button(text: string) { return [...document.querySelectorAll("button")].find(el => el.textContent?.includes(text))!; }
beforeEach(() => {
    const storage = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), get length() { return storage.size; } });
    HTMLDialogElement.prototype.showModal = function () { this.open = true; };
    HTMLDialogElement.prototype.close = function () { this.open = false; };
    host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });

describe("server project helpers", () => {
    it("explains how to enable browsing on an older agent", async () => {
        const client = createHttpClient({baseUrl:"http://agent.test", fetch: async () => new Response("Not found", {status:404})});
        await expect(listServerFolders(client)).rejects.toThrow("Choose a known project");
    });
    it("accepts old-server absolute picks unchanged but never bypasses forbidden access", async () => {
        for (const status of [404, 403]) {
            const requests: string[] = [];
            const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async input => { requests.push(String(input)); return new Response("Unavailable", {status}); } });
            if (status === 404) expect(await selectServerProject(client, "/chosen")).toBe("/chosen");
            else await expect(selectServerProject(client, "/chosen")).rejects.toMatchObject({status: 403});
            expect(requests).toHaveLength(1);
            expect(requests[0]).toContain("/v2/directories");
            await expect(selectServerProject(client, "relative")).rejects.toMatchObject({status});
        }
    });
    it("offers actual recent session directories as choices, without creating or selecting a session", async () => {
        const requests: string[] = [];
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async (input, init) => {
            expect(init?.method).toBe("GET");
            const url = new URL(String(input)); requests.push(url.pathname);
            return url.pathname === "/v2/sessions" ? Response.json({items: [{directory: "/recent"}, {directory: "/other"}], cursor: {}}) : new Response("Missing", {status: 404});
        } });
        expect(await availableProjects(client)).toEqual(["/recent", "/other"]);
        expect(requests).toEqual(["/v2/management/workspaces", "/v2/sessions"]);
    });
    it("labels Windows, UNC, Unix roots without resolving paths locally", () => {
        expect(projectName("C:\\work\\app\\")).toBe("app");
        expect(projectName("\\\\server\\share")).toBe("share");
        expect(projectName("/")).toBe("/"); expect(projectName("~/app")).toBe("app");
    });
    it("sends ~ unchanged to server without creating a session", async () => {
        const client = fakeClient(); await listServerFolders(client, "~/app");
        expect(client.operations.request).toHaveBeenCalledWith("v2.directories.list", expect.objectContaining({query: {path: "~/app"}}));
    });
    it("persists only explicit picks, scoped to server/account, most recent first", () => {
        rememberProject("server-A/user", "/a"); rememberProject("server-A/user", "/b"); rememberProject("server-A/user", "/a");
        expect(savedProjects("server-A/user")).toEqual(["/a", "/b"]); expect(savedProjects("server-B/user")).toEqual([]);
        rememberProject(undefined, "/x"); expect(savedProjects()).toEqual([]);
    });
    it("falls back to session options only when there is a session", async () => {
        const client = fakeClient(); vi.mocked(client.management.workspaces.list).mockRejectedValue(new Error("disabled"));
        expect(await availableProjects(client)).toEqual([]); expect(client.operations.request).not.toHaveBeenCalled();
        expect(await availableProjects(client, "session")).toEqual(["/fallback"]);
    });
});

describe("folder interactions", () => {
    it("hides old folder rows and disables selection while navigation is pending", async () => {
        let resolve!: (value: ReturnType<typeof listing>) => void;
        const client = fakeClient(vi.fn(({query}) => query?.path === "/root/child"
            ? new Promise(r => { resolve = r; }) : Promise.resolve(listing("/root"))));
        await act(async () => root.render(<FolderPicker client={client} directory="/root" select={vi.fn()} close={vi.fn()} />));
        await click(document.querySelector('[title="/root/child"]'), "dblclick");
        expect(document.querySelector('.project-folder-scroll [role="option"]')).toBeNull();
        expect(document.querySelector('.skeleton-folder')).not.toBeNull();
        expect(button("Select folder").disabled).toBe(true);
        await act(async () => resolve(listing("/root/child")));
        expect(document.querySelector('.skeleton-folder')).toBeNull();
        expect(button("Select folder").disabled).toBe(false);
    });
    it("single click selects, double click navigates, Back returns; only confirm commits", async () => {
        const select = vi.fn(), close = vi.fn();
        await act(async () => root.render(<FolderPicker client={fakeClient()} directory="/root" select={select} close={close} />));
        await click(document.querySelector('[title="/root/child"]'));
        expect(select).not.toHaveBeenCalled(); expect(document.querySelector('[title="/root/child"]')?.getAttribute("aria-selected")).toBe("true");
        await click(document.querySelector('[title="/root/child"]'), "dblclick");
        expect(document.querySelector('input[aria-label="Server folder path"]')?.getAttribute("value")).toBe("/root/child");
        expect(select).not.toHaveBeenCalled();
        await click(document.querySelector('[aria-label="Back"]'));
        expect(document.querySelector('input[aria-label="Server folder path"]')?.getAttribute("value")).toBe("/root");
        await click(document.querySelector('[title="/root/child"]'));
        await click(button("Select folder")); expect(select).toHaveBeenCalledWith("/root/child"); expect(close).toHaveBeenCalledOnce();
    });
    it("cancel preserves current project and does not persist browsing", async () => {
        const select = vi.fn(), close = vi.fn();
        await act(async () => root.render(<FolderPicker client={fakeClient()} select={select} close={close} />));
        await click(document.querySelector('[title="/root/child"]'));
        await click(document.querySelector('[aria-label="Close Open project"]'));
        expect(select).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledOnce(); expect(localStorage.length).toBe(0);
    });
    it("shows inaccessible folders and disables confirm", async () => {
        const client = fakeClient(vi.fn().mockRejectedValue(new Error("Forbidden")));
        await act(async () => root.render(<FolderPicker client={client} select={vi.fn()} close={vi.fn()} />));
        expect(document.querySelector('[role="alert"]')?.textContent).toContain("Forbidden");
        expect(button("Select folder").disabled).toBe(true);
    });
    it("ignores a late validation response after selecting another folder", async () => {
        let resolve!: (value: ReturnType<typeof listing>) => void;
        const client = fakeClient(vi.fn(({query}) => query?.path === "/slow" ? new Promise(r => { resolve = r; }) : Promise.resolve(listing(query?.path || "/root"))));
        await act(async () => root.render(<FolderPicker client={client} recentDirectories={["/slow"]} select={vi.fn()} close={vi.fn()} />));
        await click(document.querySelector('[title="/slow"]')); await click(document.querySelector('[title="/root/child"]'));
        await act(async () => resolve(listing("/slow")));
        expect(document.querySelector('[title="/root/child"]')?.getAttribute("aria-selected")).toBe("true");
    });
    it("retains dialog and selection when the directory PATCH fails", async () => {
        const select = vi.fn().mockRejectedValue(new Error("Update denied")), close = vi.fn();
        await act(async () => root.render(<FolderPicker client={fakeClient()} select={select} close={close} />));
        await click(button("Select folder"));
        expect(document.querySelector('[role="alert"]')?.textContent).toContain("Update denied");
        expect(close).not.toHaveBeenCalled(); expect(button("Select folder").disabled).toBe(false);
    });
    it("does not save a menu pick until the parent accepts it", async () => {
        const select = vi.fn().mockRejectedValueOnce(new Error("Update denied")).mockResolvedValue(undefined);
        await act(async () => root.render(<ProjectPicker client={fakeClient()} directory="/root" recentDirectories={["/other"]} projectStorageScope="server/user" onDirectoryChange={select} />));
        await click(document.querySelector('.project-pill'));
        await click(document.querySelector('.project-menu-row[title="/other"]'));
        expect(savedProjects("server/user")).toEqual([]);
        await click(document.querySelector('.project-menu-row[title="/other"]'));
        expect(savedProjects("server/user")).toEqual(["/other"]);
        expect(document.querySelector('.project-menu')).toBeNull();
    });
    it("commits a recent absolute pick on an old server even though browsing is unavailable", async () => {
        const select = vi.fn();
        const client = createHttpClient({baseUrl: "http://agent.test", fetch: async () => new Response("Missing", {status: 404})});
        await act(async () => root.render(<ProjectPicker client={client} recentDirectories={["/known"]} onDirectoryChange={select} />));
        await click(document.querySelector(".project-pill"));
        await click(document.querySelector('.project-menu-row[title="/known"]'));
        expect(select).toHaveBeenCalledWith("/known");
        expect(document.querySelector('[role="alert"]')).toBeNull();
    });
    it("clamps the menu to space above its actual trigger and flips below near the top", async () => {
        await act(async () => root.render(<ProjectPicker client={fakeClient()} directory="/root" onDirectoryChange={vi.fn()} />));
        const pill = document.querySelector<HTMLButtonElement>(".project-pill")!;
        let top = 350;
        vi.spyOn(pill, "getBoundingClientRect").mockImplementation(() => ({top, bottom: top + 32} as DOMRect));
        await click(pill);
        const picker = document.querySelector<HTMLDivElement>(".project-picker")!;
        expect(picker.dataset.menuPlacement).toBe("above");
        expect(picker.style.getPropertyValue("--project-menu-available")).toBe("334px");
        top = 25;
        await act(async () => { window.dispatchEvent(new Event("resize")); });
        expect(picker.dataset.menuPlacement).toBe("below");
        const height = window.visualViewport?.height ?? window.innerHeight;
        expect(picker.style.getPropertyValue("--project-menu-available")).toBe(`${height - 57 - 16}px`);
    });
    it("renders compact project menu above pill, opens folder dialog with Add project", async () => {
        await act(async () => root.render(<ProjectPicker client={fakeClient()} directory="/root" onDirectoryChange={vi.fn()} />));
        await click(document.querySelector('.project-pill'));
        expect(document.querySelector('[aria-label="Search projects"]')).not.toBeNull();
        expect(document.querySelector('[aria-current="true"]')?.textContent).toContain("root");
        await click(button("Add project"));
        expect(document.querySelector('dialog[open] h2')?.textContent).toBe("Open project");
    });
});
