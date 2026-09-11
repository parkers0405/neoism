import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { NeoismClient } from "@neoism/sdk";
import { ProviderConnections } from "./components/ProviderConnections";
import {
    authChoices,
    promptOptions,
    visiblePrompt,
    providerAccounts,
    providerOperationError,
    type Authorization,
} from "./providerConnections";

function setup() {
    const account = {
        providerId: "openai",
        connectionId: "opaque/work",
        label: "Work",
        authType: "api",
        isDefault: false,
        scope: { type: "global" },
    };
    const providers = {
        connections: vi.fn(async () => [account]),
        createConnection: vi.fn(async () => account),
        renameConnection: vi.fn(async () => account),
        deleteConnection: vi.fn(async () => true),
        setDefaultConnection: vi.fn(async () => account),
        oauthAuthorize: vi.fn(async (): Promise<Authorization | null> => ({
            method: "code",
            attemptId: "attempt-one",
            url: "https://provider.example/auth",
            instructions: "Authorize",
        })),
        oauthCallback: vi.fn(async () => true),
        setAuth: vi.fn(),
        removeAuth: vi.fn(),
    };
    const client = { catalog: { providers } } as unknown as NeoismClient;
    return {
        providers,
        client,
        api: providerAccounts(client, "openai", "workspace-one"),
    };
}

describe("provider account SDK operations", () => {
    it("lists account summaries and adds a labeled key without replacing provider auth or default", async () => {
        const { api, providers } = setup();
        expect(await api.list()).toHaveLength(1);
        expect(providers.connections).toHaveBeenCalledWith(
            "openai",
            "workspace-one",
        );
        await api.create(" Work ", " secret-key ");
        expect(providers.createConnection).toHaveBeenCalledWith(
            "openai",
            {
                label: "Work",
                credential: { type: "api", key: "secret-key" },
                setDefault: false,
            },
            "workspace-one",
        );
        expect(providers.setAuth).not.toHaveBeenCalled();
    });
    it("renames, defaults, and deletes only an opaque account ID", async () => {
        const { api, providers } = setup();
        await api.rename("opaque/work", " Renamed ");
        await api.setDefault("opaque/work");
        await api.remove("opaque/work");
        expect(providers.renameConnection).toHaveBeenCalledWith(
            "openai",
            "opaque/work",
            "Renamed",
            "workspace-one",
        );
        expect(providers.setDefaultConnection).toHaveBeenCalledWith(
            "openai",
            "opaque/work",
            "workspace-one",
        );
        expect(providers.deleteConnection).toHaveBeenCalledWith(
            "openai",
            "opaque/work",
            "workspace-one",
        );
        expect(providers.removeAuth).not.toHaveBeenCalled();
    });
    it("does not pass a directory as a workspace ID", async () => {
        const { client, providers } = setup();
        await providerAccounts(client, "openai").list();
        expect(providers.connections).toHaveBeenCalledWith("openai", undefined);
    });
    it("validates empty labels/keys before sending credentials", () => {
        const { api, providers } = setup();
        expect(() => api.create(" ", "secret")).toThrow();
        expect(() => api.create("Work", " ")).toThrow();
        expect(() => api.rename("id", " ")).toThrow();
        expect(providers.createConnection).not.toHaveBeenCalled();
        expect(providers.renameConnection).not.toHaveBeenCalled();
    });
    it("sends labels for new OAuth accounts and connectionId only for reauthorization", async () => {
        const { api, providers } = setup();
        await api.authorize(2, " Personal ", { region: "us" });
        expect(providers.oauthAuthorize).toHaveBeenLastCalledWith(
            "openai",
            { method: 2, label: "Personal", inputs: { region: "us" } },
            "workspace-one",
        );
        await api.authorize(1, "Work", {}, "opaque/work");
        expect(providers.oauthAuthorize).toHaveBeenLastCalledWith(
            "openai",
            {
                method: 1,
                label: "Work",
                inputs: {},
                connectionId: "opaque/work",
            },
            "workspace-one",
        );
    });
    it("completes code OAuth with original method and attempt, not another account ID", async () => {
        const { api, providers } = setup();
        const authorization = await api.authorize(2, "Work", {});
        await api.complete(2, authorization, " code-secret ");
        expect(providers.oauthCallback).toHaveBeenCalledWith(
            "openai",
            { method: 2, attemptId: "attempt-one", code: "code-secret" },
            "workspace-one",
        );
    });
    it("completes auto OAuth without a code", async () => {
        const { api, providers } = setup();
        await api.complete(
            0,
            {
                method: "auto",
                attemptId: "auto-attempt",
                instructions: "",
                url: "https://example.com",
            },
            "do-not-send",
        );
        expect(providers.oauthCallback).toHaveBeenCalledWith(
            "openai",
            { method: 0, attemptId: "auto-attempt" },
            "workspace-one",
        );
    });
    it("rejects null authorization, expired attempts, and missing codes", async () => {
        const { api, providers } = setup();
        providers.oauthAuthorize.mockResolvedValueOnce(null);
        await expect(api.authorize(0, "Work", {})).rejects.toThrow();
        const a = await api.authorize(0, "Work", {});
        await expect(api.complete(0, a, " ")).rejects.toThrow();
        await expect(
            api.complete(0, { ...a, expiresAt: Date.now() - 1 }, "secret"),
        ).rejects.toThrow();
        expect(providers.oauthCallback).not.toHaveBeenCalled();
    });
    it("does not report success on false callback or deletion responses", async () => {
        const { api, providers } = setup();
        providers.oauthCallback.mockResolvedValueOnce(false);
        providers.deleteConnection.mockResolvedValueOnce(false);
        await expect(
            api.complete(0, await api.authorize(0, "Work", {}), "secret"),
        ).rejects.toThrow();
        await expect(api.remove("id")).rejects.toThrow();
    });
    it("propagates failures so UI never emits a success notification", async () => {
        const { api, providers } = setup();
        providers.createConnection.mockRejectedValueOnce(
            new Error("secret echoed by backend"),
        );
        await expect(api.create("Work", "secret")).rejects.toThrow();
        expect(providerOperationError).not.toContain("secret echoed");
        expect(providerOperationError).toContain("retry");
    });
});

describe("provider picker methods and rendering", () => {
    it("preserves OAuth method indices and always offers manual API key entry", () => {
        const methods = [
            { type: "oauth" as const, label: "Browser" },
            { type: "oauth" as const, label: "Device" },
        ];
        expect(authChoices(methods).map((m) => m.label)).toEqual([
            "Browser",
            "Device",
            "Manually enter API key",
        ]);
        expect(authChoices([{ type: "api", label: "Key" }])).toHaveLength(1);
    });
    it("handles select choices and conditional provider prompts", () => {
        const prompt = {
            key: "region",
            message: "Region",
            type: "select" as const,
            options: [{ label: "US", value: "us" }, null, { value: 7 }],
            condition: { key: "mode", op: "eq", value: "custom" },
        };
        expect(promptOptions(prompt)).toEqual([{ label: "US", value: "us" }]);
        expect(visiblePrompt(prompt, {})).toBe(false);
        expect(visiblePrompt(prompt, { mode: "custom" })).toBe(true);
        expect(
            visiblePrompt(
                {
                    ...prompt,
                    condition: { key: "mode", op: "neq", value: "custom" },
                },
                {},
            ),
        ).toBe(true);
    });
    it("is backward compatible, never nests a form, and exposes only scoped account actions", () => {
        const { client } = setup();
        const html = renderToStaticMarkup(
            <ProviderConnections
                client={client}
                directory="/project"
                initialProviderId="openai"
            />,
        );
        expect(html).toContain("Add API-key account");
        expect(html).toContain('type="password"');
        expect(html).not.toContain("<form");
        expect(html).not.toContain("Disconnect provider");
        expect(html).toContain("Prompt account selection is not available");
    });
});
