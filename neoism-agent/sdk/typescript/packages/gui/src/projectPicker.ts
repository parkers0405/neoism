import { NeoismApiError, type NeoismClient } from "@neoism/sdk";
import { isAbsoluteResourceDirectory } from "./resourceScope";

export interface ServerFolder { name: string; path: string }
export interface DirectoryListing { path: string; parent: string | null; entries: ServerFolder[] }
// Paths are opaque server values. In particular, never resolve ~ or join Windows
// paths using the browser's platform, home directory, or FileSystem APIs.
export const projectName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path || "Select project";
export const uniqueProjects = (paths: string[]) => [...new Set(paths.filter(Boolean))];
export async function listServerFolders(client: NeoismClient, path?: string, signal?: AbortSignal): Promise<DirectoryListing> {
    try {
        return await client.operations.request("v2.directories.list", { query: { path: path || undefined }, signal });
    } catch (error) {
        if (error instanceof NeoismApiError && error.status === 404) {
            throw new Error("Folder browsing is unavailable on this server. Choose a known project or enter an absolute server path instead.");
        }
        throw error;
    }
}
const storageKey = (scope: string) => `neoism.project-picks.v1:${scope}`;
export function savedProjects(scope?: string): string[] {
    if (!scope) return [];
    try { const value: unknown = JSON.parse(localStorage.getItem(storageKey(scope)) || "[]"); return Array.isArray(value) ? uniqueProjects(value.filter((p): p is string => typeof p === "string")).slice(0, 20) : []; } catch { return []; }
}
export function rememberProject(scope: string | undefined, path: string): void {
    if (!scope) return;
    try { localStorage.setItem(storageKey(scope), JSON.stringify(uniqueProjects([path, ...savedProjects(scope)]).slice(0, 20))); } catch { /* Browsing still works without storage. */ }
}
export async function availableProjects(client: NeoismClient, sessionId?: string): Promise<string[]> {
    try { return uniqueProjects((await client.management.workspaces.list()).map(p => p.root)); }
    catch (error) {
        if (error instanceof NeoismApiError && [401, 403].includes(error.status)) return [];
        // Recents are choices only, never an inferred current/default root.
        // No session is created to obtain directory options.
        if (!sessionId) {
            try { return uniqueProjects((await client.sessions.list({ limit: 100, roots: true })).items.map(session => session.directory)).filter(isAbsoluteResourceDirectory); }
            catch { return []; }
        }
        try { return uniqueProjects(await client.operations.request("v2.sessions.directoryOptions", { path: { session_id: sessionId }, query: { limit: 100 } })); }
        catch { return []; }
    }
}

/** Only missing browsing support permits opaque absolute-path compatibility.
 * In particular 401/403 must never be converted into a successful selection. */
export async function selectServerProject(client: NeoismClient, path: string): Promise<string> {
    try {
        const result = await client.operations.request("v2.directories.list", { query: { path } });
        if (!isAbsoluteResourceDirectory(result.path)) throw new Error("Choose an absolute server path.");
        return result.path;
    } catch (error) {
        if (error instanceof NeoismApiError && error.status === 404 && isAbsoluteResourceDirectory(path)) return path;
        throw error;
    }
}
