// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { NeoismClient } from "@neoism/sdk";
import { TypeSafeComputerSettings } from "./TypeSafeComputerSettings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const roots: ReturnType<typeof createRoot>[] = [];
afterEach(async () => { for (const root of roots.splice(0)) await act(async () => root.unmount()); document.body.innerHTML = ""; });

async function setup(enabled = false) {
    const config = { get: vi.fn(async () => ({ model: "openai/existing-model", experimental: { options: { "unrelated-feature": true, "computer-typesafe": { enabled } } } })), update: vi.fn(async () => ({})) };
    const providers = { setAuth: vi.fn(async () => true), removeAuth: vi.fn(async () => true) };
    const client = { config, catalog: { providers } } as unknown as NeoismClient;
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host); roots.push(root);
    await act(async () => root.render(<TypeSafeComputerSettings client={client} directory="/workspace" />));
    return { host, config, providers };
}

function button(host: HTMLElement, label: string) { return [...host.querySelectorAll("button")].find(button => button.textContent === label)!; }

describe("TypeSafe computer settings", () => {
    it("hides key entry until enabled and stores only the flag in configuration", async () => {
        const { host, config } = await setup();
        expect(host.querySelector('input[type="password"]')).toBeNull();
        await act(async () => (host.querySelector('input[type="checkbox"]') as HTMLInputElement).click());
        expect(config.update).toHaveBeenCalledWith({ model: "openai/existing-model", experimental: { options: { "unrelated-feature": true, "computer-typesafe": { enabled: true } } } }, "/workspace");
        expect(host.querySelector('input[type="password"]')).not.toBeNull();
        expect(host.textContent).toContain("sent to TypeSafe");
    });
    it("saves keys through the credential API, clears the field, and never patches secrets into config", async () => {
        const { host, config, providers } = await setup(true);
        const input = host.querySelector('input[type="password"]') as HTMLInputElement;
        await act(async () => {
            Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, " test-secret ");
            input.dispatchEvent(new Event("input", { bubbles: true }));
        });
        await act(async () => button(host, "Save TypeSafe key").click());
        expect(providers.setAuth).toHaveBeenCalledWith("typesafe", { type: "api", key: "test-secret" });
        expect(config.update).not.toHaveBeenCalled();
        expect(input.value).toBe("");
        expect(host.textContent).not.toContain("test-secret");
        await act(async () => button(host, "Remove saved TypeSafe key").click());
        expect(providers.removeAuth).toHaveBeenCalledWith("typesafe");
    });
    it("does not render a returned backend error containing a secret", async () => {
        const { host, providers } = await setup(true);
        providers.removeAuth.mockRejectedValue(new Error("test-secret"));
        await act(async () => button(host, "Remove saved TypeSafe key").click());
        expect(host.textContent).toContain("Could not remove");
        expect(host.textContent).not.toContain("test-secret");
    });
});
