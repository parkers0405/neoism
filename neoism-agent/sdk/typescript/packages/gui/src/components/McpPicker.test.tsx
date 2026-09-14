// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { NeoismClient } from "@neoism/sdk";
import type { McpCatalog } from "../mcpActions";
import { McpPicker } from "./McpPicker";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

it("labels actual global/workspace owners and keeps read-only mutation disabled", async () => {
    const entry = (configScope: "global" | "workspace", configWritable = true): McpCatalog[string] => ({
        configScope, configWritable, enabled: false, runtimeConnected: false,
        oauthCapable: false, hasCredentials: false, status: { status: "disabled" },
    });
    const catalog: McpCatalog = {
        global: entry("global"), local: entry("workspace"),
        lockedGlobal: entry("global", false), lockedLocal: entry("workspace", false),
    };
    const plugin = { catalog: vi.fn(async () => catalog), configure: vi.fn(async () => {}) };
    const client = { plugins: { use: vi.fn(async () => plugin) } } as unknown as NeoismClient;
    const host = document.createElement("div");
    const root = createRoot(host);
    try {
        await act(async () => root.render(<McpPicker client={client} directory="/fixture/workspace" />));
        const rows = [...host.querySelectorAll("article")];
        expect(rows).toHaveLength(4);
        for (const [index, label] of ["Global", "Workspace", "Global (read-only)", "Workspace (read-only)"].entries()) {
            expect(rows[index].textContent).toContain(label);
            const toggle = rows[index].querySelector("button")!;
            expect(toggle.disabled).toBe(index >= 2);
            expect(toggle.title).toContain(index % 2 === 0 ? "Global config" : "Workspace config");
        }
        await act(async () => rows[0].querySelector("button")!.click());
        expect(plugin.configure).toHaveBeenCalledWith("global", { enabled: true }, "/fixture/workspace");
        expect(plugin.catalog).toHaveBeenCalledWith("/fixture/workspace");
    } finally {
        await act(async () => root.unmount());
    }
});
