import { useEffect, useState } from "react";
import type { NeoismClient } from "@neoism/sdk";

/** Server process identity, not an authenticated collaborator/account identity. */
export interface ServerIdentity { configuredName?: string | null; systemName?: string | null }
const clean = (value: unknown) => typeof value === "string" ? value.trim() : "";
export function resolveIdentity(configured: string, server?: ServerIdentity): string {
    // "You" was persisted automatically by older GUIs, not a discovered identity.
    const local = clean(configured);
    return (local !== "You" ? local : "") || clean(server?.configuredName) || clean(server?.systemName) || "You";
}
export function messageAuthor(info: { author?: unknown }, localName: string): string {
    // Missing metadata is the legacy local-message fallback. Explicit anonymous
    // or malformed attribution is not evidence of local ownership.
    return clean(info.author) || (info.author === undefined ? localName : "Anonymous user");
}
export function useIdentity(client: NeoismClient, configured: string, local = true): string {
    const [result, setResult] = useState<{ client: NeoismClient; value: ServerIdentity }>();
    useEffect(() => {
        if (!local) return;
        const abort = new AbortController();
        // Older deployments have no identity endpoint. Never infer an OS user
        // from location.hostname, directory paths, or browser environment.
        void client.transport?.request<ServerIdentity>({ path: "/v2/identity", signal: abort.signal })
            .then(value => { if (!abort.signal.aborted) setResult({ client, value }); })
            .catch(() => { /* Explicit browser name or honest unknown fallback. */ });
        return () => abort.abort();
    }, [client, local]);
    return resolveIdentity(configured, local && result?.client === client ? result.value : undefined);
}
