// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { NeoismClient } from "@neoism/sdk";
import { useIdentity, type ServerIdentity } from "./identity";
(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
afterEach(() => vi.restoreAllMocks());
function Profile({ client, configured = "" }: { client: NeoismClient; configured?: string }) {
    return <span>{useIdentity(client, configured)}</span>;
}
it("uses server metadata and isolates stale/failed identity requests across server changes", async () => {
    let resolve!: (value: ServerIdentity) => void;
    const request = vi.fn(() => new Promise<ServerIdentity>(r => { resolve = r; }));
    const first = { transport: { request } } as unknown as NeoismClient;
    const secondRequest = vi.fn(() => Promise.reject(new Error("404: older deployment")));
    const second = { transport: { request: secondRequest } } as unknown as NeoismClient;
    const host = document.createElement("div"), root = createRoot(host);
    try {
        await act(async () => root.render(<Profile client={first} />));
        expect(host.textContent).toBe("You");
        await act(async () => resolve({configuredName: "Native", systemName: "server-user"}));
        expect(host.textContent).toBe("Native");
        await act(async () => root.render(<Profile client={second} />));
        expect(host.textContent).toBe("You");
        expect((request.mock.calls as unknown as [{signal: AbortSignal}][])[0][0].signal.aborted).toBe(true);
        await act(async () => root.render(<Profile client={second} configured="Explicit" />));
        expect(host.textContent).toBe("Explicit");
        expect(secondRequest).toHaveBeenCalledTimes(1);
    } finally { act(() => root.unmount()); }
});
it("does not publish a previous server's late response", async () => {
    let resolve!: (value: ServerIdentity) => void;
    const first = { transport: { request: () => new Promise<ServerIdentity>(r => { resolve = r; }) } } as unknown as NeoismClient;
    const second = { transport: { request: () => Promise.resolve({ systemName: "second-user" }) } } as unknown as NeoismClient;
    const host = document.createElement("div"), root = createRoot(host);
    try {
        await act(async () => root.render(<Profile client={first} />));
        await act(async () => root.render(<Profile client={second} />));
        await act(async () => resolve({ systemName: "wrong-user" }));
        expect(host.textContent).toBe("second-user");
    } finally { act(() => root.unmount()); }
});
