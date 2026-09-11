import { useEffect, useRef, useState } from "react";
import type { NeoismClient } from "@neoism/sdk";
import { getMcp, runMcpAction, type McpCatalog, type McpAction } from "../mcpActions";

export function McpPicker({ client, directory }: { client: NeoismClient; directory: string }) {
    const [catalog, setCatalog] = useState<McpCatalog>({});
    const [busy, setBusy] = useState(true);
    const [error, setError] = useState("");
    const [auth, setAuth] = useState<{ name: string; url: string }>();
    const [code, setCode] = useState("");
    const alive = useRef(true);
    useEffect(() => {
        alive.current = true;
        void refresh();
        return () => { alive.current = false; };
    }, []);
    async function refresh() {
        try {
            const plugin = await getMcp(client, directory);
            const next = await plugin.catalog(directory);
            if (alive.current) setCatalog(next);
        } catch (e) { if (alive.current) setError(String(e)); }
        finally { if (alive.current) setBusy(false); }
    }
    async function act(name: string, action: McpAction) {
        setBusy(true); setError(""); setAuth(undefined);
        try {
            const plugin = await getMcp(client, directory);
            if (!alive.current) return;
            const result = await runMcpAction(plugin, directory, name, action);
            if (!alive.current) return;
            if (result) setAuth({ name, url: result.authorizationUrl });
            await refresh();
        } catch (e) { if (alive.current) setError(String(e)); }
        finally { if (alive.current) setBusy(false); }
    }
    async function completeAuth() {
        if (!auth || !code.trim()) return;
        setBusy(true); setError("");
        try {
            const plugin = await getMcp(client, directory);
            if (!alive.current) return;
            await plugin.submitAuthCode(auth.name, code.trim(), directory);
            if (!alive.current) return;
            setAuth(undefined); setCode(""); await refresh();
        } catch (e) { if (alive.current) setError(String(e)); }
        finally { if (alive.current) setBusy(false); }
    }
    return <section className="mcp-picker" aria-label="MCP server catalog" aria-busy={busy}>
        <p>Workspace: {directory || "Server default"}</p>
        <button disabled={busy} onClick={() => { setBusy(true); void refresh(); }}>Refresh</button>
        {error && <p role="alert">{error}</p>}
        {!busy && !Object.keys(catalog).length && <p>No MCP servers configured in this workspace.</p>}
        {Object.entries(catalog).map(([name, entry]) => <article key={name}>
            <h3>{name}</h3>
            <p>{entry.enabled ? "Enabled" : "Disabled"} · {entry.runtimeConnected ? "Connected" : "Disconnected"} · {entry.status.status}</p>
            {"error" in entry.status && <p role="status">{String(entry.status.error)}</p>}
            {!entry.configWritable && <small>Read-only configuration; runtime connections and credentials can still be managed.</small>}
            <div className="mcp-actions">
                <button disabled={busy || !entry.configWritable} onClick={() => void act(name, entry.enabled ? "disable" : "enable")}>{entry.enabled ? "Disable" : "Enable"}</button>
                <button disabled={busy || (!entry.enabled && !entry.runtimeConnected)} onClick={() => void act(name, entry.runtimeConnected ? "disconnect" : "connect")}>{entry.runtimeConnected ? "Disconnect" : "Connect"}</button>
                {entry.oauthCapable && <button disabled={busy} onClick={() => void act(name, "auth")}>Authenticate</button>}
                {entry.hasCredentials && <button disabled={busy} onClick={() => void act(name, "logout")}>Log out</button>}
            </div>
        </article>)}
        {auth && <div className="mcp-auth"><h3>Authenticate {auth.name}</h3>
            <a href={/^https?:\/\//i.test(auth.url) ? auth.url : undefined} target="_blank" rel="noreferrer">Open authorization page</a>
            <p>After authorization, refresh the catalog. If the provider gives you a code, submit it here.</p>
            <input aria-label="Authorization code" value={code} onChange={(e) => setCode(e.target.value)} />
            <button disabled={busy || !code.trim()} onClick={() => void completeAuth()}>Submit code</button>
        </div>}
    </section>;
}
