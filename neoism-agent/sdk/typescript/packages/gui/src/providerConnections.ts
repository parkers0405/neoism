import type { NeoismClient, OperationResponse } from "@neoism/sdk";

export type ProviderConnection =
    OperationResponse<"v2.providers.connections.list">[number];
/** On deletion, clear/invalidate only a matching selection; never silently bill a fallback. */
export type ProviderConnectionSelection = {
    providerId: string;
    connectionId: string;
    label: string;
    reason: "selected" | "deleted";
};
export type ProviderConnectionPickerProps = {
    onSelectConnection?: (selection: ProviderConnectionSelection) => void;
    selectedConnection?: { providerId: string; connectionId: string };
    initialProviderId?: string;
    workspaceId?: string;
};
export type Authorization = NonNullable<
    OperationResponse<"v2.providers.oauth.authorize">
>;
export type AuthMethod =
    OperationResponse<"v2.providers.authMethods">[string][number];
export const providerOperationError =
    "The account operation failed. Check the server connection and authorization, then retry. Credentials and server error details are not displayed.";

export function authChoices(methods: AuthMethod[] = []): AuthMethod[] {
    // Preserve server method indices; manual API entry doesn't call an auth method endpoint.
    return methods.some((m) => m.type === "api")
        ? methods
        : [...methods, { type: "api", label: "Manually enter API key" }];
}
export function promptOptions(
    prompt: NonNullable<AuthMethod["prompts"]>[number],
) {
    if (!Array.isArray(prompt.options)) return [];
    return prompt.options.flatMap((o: unknown) => {
        if (!o || typeof o !== "object") return [];
        const option = o as Record<string, unknown>;
        return typeof option.value === "string" &&
            typeof option.label === "string"
            ? [{ value: option.value, label: option.label }]
            : [];
    });
}
export function visiblePrompt(
    prompt: NonNullable<AuthMethod["prompts"]>[number],
    inputs: Record<string, string>,
) {
    const condition = prompt.condition as
        { key?: string; op?: string; value?: string } | undefined;
    if (!condition?.key) return true;
    return condition.op === "neq"
        ? inputs[condition.key] !== condition.value
        : inputs[condition.key] === condition.value;
}

/** Only opaque account IDs reach mutations. No provider-wide auth deletion/replacement. */
export function providerAccounts(
    client: NeoismClient,
    providerId: string,
    workspaceId?: string,
) {
    const api = client.catalog.providers;
    return {
        list: () => api.connections(providerId, workspaceId),
        create: (label: string, key: string) => {
            if (!label.trim() || !key.trim())
                throw new Error("Account label and API key are required.");
            return api.createConnection(
                providerId,
                {
                    label: label.trim(),
                    credential: { type: "api", key: key.trim() },
                    setDefault: false,
                },
                workspaceId,
            );
        },
        rename: (id: string, label: string) => {
            if (!label.trim()) throw new Error("Account label is required.");
            return api.renameConnection(
                providerId,
                id,
                label.trim(),
                workspaceId,
            );
        },
        remove: async (id: string) => {
            if (!(await api.deleteConnection(providerId, id, workspaceId)))
                throw new Error("Deletion failed");
        },
        setDefault: (id: string) =>
            api.setDefaultConnection(providerId, id, workspaceId),
        authorize: async (
            method: number,
            label: string,
            inputs: Record<string, string>,
            connectionId?: string,
        ) => {
            if (!label.trim()) throw new Error("Account label is required.");
            const result = await api.oauthAuthorize(
                providerId,
                {
                    method,
                    label: label.trim(),
                    inputs,
                    ...(connectionId ? { connectionId } : {}),
                },
                workspaceId,
            );
            if (!result) throw new Error("Authorization unavailable");
            return result;
        },
        complete: async (
            method: number,
            authorization: Authorization,
            code: string,
        ) => {
            if (
                authorization.expiresAt &&
                authorization.expiresAt <= Date.now()
            )
                throw new Error("Authorization expired");
            if (authorization.method === "code" && !code.trim())
                throw new Error("Authorization code required");
            const ok = await api.oauthCallback(
                providerId,
                {
                    method,
                    attemptId: authorization.attemptId,
                    ...(authorization.method === "code"
                        ? { code: code.trim() }
                        : {}),
                },
                workspaceId,
            );
            if (!ok) throw new Error("Authorization incomplete");
        },
    };
}
