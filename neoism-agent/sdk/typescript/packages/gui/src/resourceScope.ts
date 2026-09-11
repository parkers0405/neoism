import { useCallback, useEffect, useMemo, useState } from "react";
import { NeoismApiError, type NeoismClient } from "@neoism/sdk";

export interface ResourceScope {
    directory?: string;
    loading: boolean;
    error?: string;
    needsSelection?: boolean;
    pathNote?: string;
    canManage: boolean;
    managementReason?: string;
    retry(): void;
}
type Resolution = Omit<ResourceScope, "retry" | "loading">;
// Recognition only: server paths must never be normalized using browser semantics.
export function isAbsoluteResourceDirectory(path: string): boolean {
    return path.startsWith("/") || /^[a-z]:[\\/]/i.test(path) || /^\\\\[^\\]+\\[^\\]+/.test(path);
}
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
const authority = "Management is available; the server still requires operator authorization for writes.";

export async function resolveResourceScope(client: NeoismClient, selected?: string, signal?: AbortSignal): Promise<Resolution> {
    let directory: string;
    let pathNote: string | undefined;
    try {
        const result = await client.operations.request("v2.directories.list", { query: { path: selected || undefined }, signal });
        if (!isAbsoluteResourceDirectory(result.path)) throw new Error("The server did not return an absolute project directory. Choose an absolute server path.");
        directory = result.path;
    } catch (error) {
        if (!(error instanceof NeoismApiError && error.status === 404)) {
            return { canManage: false, error: error instanceof NeoismApiError && [401, 403].includes(error.status) ? `Project access denied: ${message(error)}` : `Cannot open this project: ${message(error)}` };
        }
        if (!selected || !isAbsoluteResourceDirectory(selected)) {
            return { canManage: false, needsSelection: true };
        }
        directory = selected;
        pathNote = "Using your absolute server path unchanged. Folder browsing is unavailable on this server; resource requests remain subject to server authorization and path checks.";
    }
    try {
        const capabilities = await client.capabilities.list(directory);
        const management = capabilities.find(item => item.id === "neoism.management");
        const canManage = management?.enabled === true;
        return { directory, canManage, pathNote, managementReason: canManage ? authority : management?.reason || "Resource management is not enabled on this server. Enable server management and connect with operator authorization to create resources." };
    } catch (error) {
        return { directory, canManage: false, pathNote, managementReason: `Management availability is unknown: ${message(error)}` };
    }
}

export async function resolveGlobalResourceScope(client: NeoismClient): Promise<Resolution> {
    try {
        const capabilities = await client.operations.request("v2.capabilities.list", { query: { scope: "installation" } });
        if (!capabilities.some(item => item.id === "neoism.resources.installation" && item.enabled)) {
            return { canManage: false, error: "Global resources require an updated Agent backend. Rebuild the backend to use this Library." };
        }
        const management = capabilities.find(item => item.id === "neoism.management");
        const canManage = management?.enabled === true;
        return { canManage, managementReason: canManage ? undefined : management?.reason || "Resource management is not enabled on this server." };
    } catch (error) {
        return { canManage: false, error: `Cannot check global resource support: ${message(error)}` };
    }
}

/** Global Library context is independent of project selection. */
export function useResourceScope(client: NeoismClient): ResourceScope {
    const [attempt, setAttempt] = useState(0);
    const identity = useMemo(() => ({ client, attempt }), [client, attempt]);
    const [state, setState] = useState<{ identity: typeof identity; value: Resolution }>();
    const retry = useCallback(() => setAttempt(value => value + 1), []);
    useEffect(() => {
        const controller = new AbortController();
        let active = true;
        void resolveGlobalResourceScope(client).then(value => {
            if (active) setState({ identity, value });
        });
        return () => { active = false; controller.abort(); };
    }, [client, identity]);
    // Render-time identity check prevents even one frame exposing the prior root,
    // including switching A -> B -> A while requests are pending.
    if (!state || state.identity !== identity) {
        return { loading: true, canManage: false, managementReason: "Checking server capabilities…", retry };
    }
    return { ...state.value, loading: false, retry };
}
