// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it } from "vitest";
import { createHttpClient } from "@neoism/sdk";
import { Library } from "./components/Library";
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

const globalCapability = { id: "neoism.resources.installation", enabled: true };
describe("Global Library", () => {
    it.each([undefined, "", "/selected/project"])("loads global resources without a project gate (%s)", async directory => {
        const requests: URL[] = [];
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async input => {
            const url = new URL(String(input)); requests.push(url);
            if (url.pathname === "/v2/capabilities") return Response.json([globalCapability, { id: "neoism.management", enabled: false, reason: "Operator management is disabled." }]);
            if (url.pathname === "/v2/management/skills") return Response.json([]);
            throw Error(`Unexpected request: ${url}`);
        } });
        const host = document.createElement("div"), root = createRoot(host);
        try {
            await act(async () => root.render(<Library kind="skills" client={client} directory={directory} openSession={() => {}} />));
            expect(host.textContent).toContain("Global skills");
            expect(host.textContent).not.toMatch(/Choose a project|No project selected/);
            expect(host.querySelector(".project-pill")).toBeNull();
            expect(host.querySelector<HTMLButtonElement>(".page-heading .primary")?.disabled).toBe(true);
            expect(requests).toHaveLength(2);
            for (const url of requests) { expect(url.searchParams.get("scope")).toBe("installation"); expect(url.searchParams.has("directory")).toBe(false); }
            await act(async () => root.render(<Library kind="skills" client={client} directory="/different" openSession={() => {}} />));
            expect(requests).toHaveLength(2);
        } finally { await act(async () => root.unmount()); }
    });
    it("does not pretend an old backend's workspace results are global", async () => {
        const requests: URL[] = [];
        const client = createHttpClient({ baseUrl: "http://old.test", fetch: async input => { requests.push(new URL(String(input))); return Response.json([{ id: "neoism.management", enabled: true }]); } });
        const host = document.createElement("div"), root = createRoot(host);
        try {
            await act(async () => root.render(<Library kind="skills" client={client} openSession={() => {}} />));
            expect(host.textContent).toContain("updated Agent backend");
            expect(requests).toHaveLength(1);
            expect(host.querySelector(".page-heading .primary")).toBeNull();
        } finally { await act(async () => root.unmount()); }
    });
    it("opens a global create form without a selected directory and retains it when project changes", async () => {
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async input => Response.json(new URL(String(input)).pathname === "/v2/capabilities" ? [globalCapability, { id: "neoism.management", enabled: true }] : []) });
        const host = document.createElement("div"), root = createRoot(host); document.body.append(host);
        try {
            await act(async () => root.render(<Library kind="skills" client={client} openSession={() => {}} />));
            await act(async () => host.querySelector<HTMLButtonElement>(".page-heading .primary")!.click());
            const form = document.querySelector(".resource-editor");
            expect(document.body.textContent).toContain("Global skills");
            expect(document.querySelector<HTMLButtonElement>('.resource-editor-actions button[type="submit"]')?.disabled).toBe(false);
            expect(document.body.textContent).not.toContain("workspace by default");
            await act(async () => root.render(<Library kind="skills" client={client} directory="/unrelated" openSession={() => {}} />));
            expect(document.querySelector(".resource-editor")).toBe(form);
        } finally { await act(async () => root.unmount()); host.remove(); }
    });
});
