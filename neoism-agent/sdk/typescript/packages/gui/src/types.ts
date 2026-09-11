import { DEFAULT_GUI_THEME } from "./appearance";

export interface Theme {
    id: string;
    name: string;
    colors: Record<string, string>;
}
export interface SlashCommand {
    name: string;
    description: string;
    aliases: string[];
}
export interface Preferences {
    name: string;
    server: string;
    directory: string;
    theme: string;
    font: string;
    codeFont?: string;
}
export const defaultPreferences: Preferences = {
    name: "You",
    server: import.meta.env.VITE_NEOISM_AGENT_URL || (
        import.meta.env.DEV || typeof location === "undefined"
            ? "http://127.0.0.1:4096"
            : location.origin
    ),
    directory: "",
    theme: DEFAULT_GUI_THEME,
    font: "geist",
    codeFont: "jetbrains-mono",
};
export function loadPreferences(): Preferences {
    try {
        const saved: unknown = JSON.parse(
            localStorage.getItem("neoism.gui.preferences") || "{}",
        );
        if (!saved || typeof saved !== "object") return defaultPreferences;
        const result = { ...defaultPreferences };
        for (const key of Object.keys(result) as (keyof Preferences)[]) {
            const value = (saved as Record<string, unknown>)[key];
            if (typeof value === "string") result[key] = value;
        }
        if (result.theme === "neoism") result.theme = DEFAULT_GUI_THEME;
        return result;
    } catch {
        return defaultPreferences;
    }
}
export function loadAccounts(server: string): Record<string, string | null> {
    try {
        const saved: unknown = JSON.parse(localStorage.getItem("neoism.gui.accounts:" + server) || "{}");
        if (!saved || typeof saved !== "object" || Array.isArray(saved)) return {};
        return Object.fromEntries(Object.entries(saved).filter(([, value]) => value === null || typeof value === "string"));
    } catch { return {}; }
}
export function saveAccounts(server: string, accounts: Record<string, string | null>): void {
    try { localStorage.setItem("neoism.gui.accounts:" + server, JSON.stringify(accounts)); } catch { /* Visit-local selection still works. */ }
}
export function rememberedSession(server: string): string | undefined {
    try {
        const params = new URLSearchParams(window.location.hash.slice(1));
        if (params.get("server") === server && params.has("session"))
            return params.get("session") || undefined;
        return localStorage.getItem("neoism.gui.session:" + server) || undefined;
    } catch { return undefined; }
}
export function rememberSession(server: string, id?: string, push = false, tabKey?: string): void {
    try {
        if (id) localStorage.setItem("neoism.gui.session:" + server, id);
        else localStorage.removeItem("neoism.gui.session:" + server);
        const url = new URL(window.location.href);
        const params = new URLSearchParams(url.hash.slice(1));
        params.set("server", server);
        params.set("session", id || "");
        if (tabKey) params.set("tab", tabKey); else params.delete("tab");
        url.hash = params.toString();
        if (url.href !== window.location.href)
            window.history[push ? "pushState" : "replaceState"](null, "", url);
    } catch { /* Private browsing may disallow persistence/history. */ }
}
export function errorMessage(error: unknown): string {
    const e = (
        error && typeof error === "object" ? error : { message: String(error) }
    ) as { status?: number; message?: string };
    if (e.status === 401 || e.status === 403)
        return `Authorization required. Check your server token and resource permissions in Settings. ${e.message || ""}`;
    if (e.status === 404 || e.status === 501)
        return `This server does not expose this capability. Enable the relevant plugin / management API on the server. ${e.message || ""}`;
    if (e.status === 409 || e.status === 412)
        return "This resource changed on the server. Reload before saving again.";
    if (e.message === "Failed to fetch" || e.message?.includes("NetworkError"))
        return "Cannot reach the agent server. Start neoism-agent web --no-open, verify the URL in Settings, and check HTTPS / CORS when connecting remotely.";
    return e.message || String(error);
}

/** Credential identity: URL parser canonicalizes host/default port; paths remain scoped. */
export function serverScope(server: string): string {
    try { const url = new URL(server); url.hash = ""; return url.href.replace(/\/+$/, ""); }
    catch { return server.trim().replace(/\/+$/, ""); }
}
export function loadDeletedAccounts(server: string): Record<string, string[]> {
    try {
        const value: unknown = JSON.parse(localStorage.getItem("neoism.gui.deleted-accounts:" + serverScope(server)) || "{}");
        if (!value || typeof value !== "object" || Array.isArray(value)) return {};
        return Object.fromEntries(Object.entries(value).filter(([, ids]) => Array.isArray(ids) && ids.every(id => typeof id === "string")));
    } catch { return {}; }
}
export function saveDeletedAccounts(server: string, deleted: Record<string, string[]>): void {
    try { localStorage.setItem("neoism.gui.deleted-accounts:" + serverScope(server), JSON.stringify(deleted)); } catch { /* Visit-local blocking remains enforced. */ }
}
