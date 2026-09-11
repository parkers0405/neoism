// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it } from "vitest";
import { createHttpClient, type NeoismClient } from "@neoism/sdk";
import { isAbsoluteResourceDirectory, resolveResourceScope, useResourceScope, type ResourceScope } from "./resourceScope";
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
function clientFor(browseStatus = 200, capabilities: unknown = [{ id: "neoism.management", enabled: true }], capabilityStatus = 200) {
    const urls: URL[] = [];
    const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async input => {
        const url = new URL(String(input)); urls.push(url);
        return url.pathname === "/v2/directories"
            ? Response.json({ path: "/selected/canonical", entries: [], parent: "/selected" }, { status: browseStatus })
            : Response.json(capabilities, { status: capabilityStatus });
    } });
    return { client, urls };
}
describe("resource scope", () => {
    it("resolves empty and relative paths only on the server and scopes capability requests", async () => {
        for (const path of [undefined, "", "../selected"]) {
            const { client, urls } = clientFor();
            expect(await resolveResourceScope(client, path)).toMatchObject({ directory: "/selected/canonical", canManage: true });
            expect(urls[0].searchParams.get("path")).toBe(path || null);
            expect(urls[1].searchParams.get("directory")).toBe("/selected/canonical");
        }
    });
    it("keeps absolute old-server reads usable without substituting the default project", async () => {
        const { client, urls } = clientFor(404);
        expect(await resolveResourceScope(client, "/actual/project")).toMatchObject({ directory: "/actual/project", canManage: true, pathNote: expect.stringContaining("unchanged") });
        expect(urls[1].searchParams.get("directory")).toBe("/actual/project");
        expect((await resolveResourceScope(client, "/actual/project")).error).toBeUndefined();
        for (const path of [undefined, "", "relative", "~/project", "C:relative"]) {
            expect(await resolveResourceScope(client, path)).toMatchObject({ canManage: false, needsSelection: true });
            expect((await resolveResourceScope(client, path)).directory).toBeUndefined();
        }
    });
    it("never falls back on forbidden, missing folder or network failures", async () => {
        for (const status of [400, 401, 403, 500]) {
            const { client, urls } = clientFor(status);
            expect(await resolveResourceScope(client, "/absolute")).toMatchObject({ canManage: false });
            expect(urls).toHaveLength(1);
        }
    });
    it("requires enabled real management capability, not plugin availability or a token", async () => {
        for (const caps of [[], [{ id: "neoism.workflows", enabled: true }], [{ id: "neoism.management", enabled: false }]]) {
            expect(await resolveResourceScope(clientFor(200, caps).client)).toMatchObject({ canManage: false, directory: "/selected/canonical" });
        }
        expect(await resolveResourceScope(clientFor(200, {}, 403).client)).toMatchObject({ canManage: false, directory: "/selected/canonical", managementReason: expect.stringContaining("unknown") });
    });
    it("recognizes opaque POSIX, Windows and UNC paths without local normalization", () => {
        for (const path of ["/project", "C:\\project", "D:/project", "\\\\host\\share\\project"]) expect(isAbsoluteResourceDirectory(path)).toBe(true);
        for (const path of ["", ".", "../project", "~/project", "C:project", "\\project"]) expect(isAbsoluteResourceDirectory(path)).toBe(false);
    });
    it("guards pending global requests by client identity and retries", async () => {
        const host = document.createElement("div"), root = createRoot(host);
        const pending: Array<(value: unknown) => void> = [];
        const slow = { operations: { request: () => new Promise(resolve => pending.push(resolve)) } } as unknown as NeoismClient;
        const caps = [{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: true }];
        let scope!: ResourceScope;
        function Probe({ client }: {client: NeoismClient}) { scope = useResourceScope(client); return null; }
        try {
            await act(async () => root.render(<Probe client={slow} />));
            await act(async () => scope.retry());
            await act(async () => pending[0](caps));
            expect(scope.loading).toBe(true); expect(scope.directory).toBeUndefined();
            await act(async () => pending[1](caps));
            expect(scope.canManage).toBe(true); expect(scope.directory).toBeUndefined();
            await act(async () => scope.retry());
            expect(scope.canManage).toBe(false);
            await act(async () => root.render(<Probe client={clientFor(200, caps).client} />));
            await act(async () => pending[2]([]));
            expect(scope.canManage).toBe(true); expect(scope.directory).toBeUndefined();
        } finally { await act(async () => root.unmount()); }
    });
});
