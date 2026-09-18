import { useEffect, useState } from "react";
import type { NeoismClient } from "@neoism/sdk";

export function TypeSafeComputerSettings({ client, directory }: { client: NeoismClient; directory: string }) {
    const [enabled, setEnabled] = useState(false);
    const [ready, setReady] = useState(false);
    const [busy, setBusy] = useState(false);
    const [key, setKey] = useState("");
    const [notice, setNotice] = useState("");
    useEffect(() => {
        let active = true;
        setKey(""); setReady(false); setEnabled(false); setNotice("");
        void client.config.get(directory).then(config => {
            const settings = config as { experimental?: { options?: { "computer-typesafe"?: { enabled?: boolean } } } };
            if (active) { setEnabled(settings.experimental?.options?.["computer-typesafe"]?.enabled === true); setReady(true); }
        }).catch(() => { if (active) setNotice("Could not load experimental computer settings."); });
        return () => { active = false; };
    }, [client, directory]);

    async function toggle(next: boolean) {
        setBusy(true); setNotice("");
        try {
            // The config API replaces the document; preserve unrelated settings.
            const config = await client.config.get(directory);
            const experimental = (config.experimental ?? {}) as Record<string, unknown>;
            const options = (experimental.options ?? {}) as Record<string, unknown>;
            const mode = (options["computer-typesafe"] ?? {}) as Record<string, unknown>;
            await client.config.update({ ...config, experimental: { ...experimental, options: { ...options, "computer-typesafe": { ...mode, enabled: next } } } }, directory);
            setEnabled(next); setKey("");
        } catch { setNotice("Could not update TypeSafe mode. Check configuration write access."); }
        finally { setBusy(false); }
    }
    async function saveKey() {
        if (!key.trim()) return;
        setBusy(true); setNotice("");
        try {
            await client.catalog.providers.setAuth("typesafe", { type: "api", key: key.trim() });
            setNotice("TypeSafe key saved on the server. It is not stored in workspace config. Key validity is checked on the first request.");
        } catch { setNotice("Could not save the TypeSafe key. Check server authorization."); }
        finally { setKey(""); setBusy(false); }
    }
    async function removeKey() {
        setBusy(true); setNotice("");
        try {
            await client.catalog.providers.removeAuth("typesafe");
            setNotice("Saved TypeSafe credential removed. A server TYPESAFE_API_KEY environment variable, if set, still takes precedence.");
        } catch { setNotice("Could not remove the TypeSafe credential."); }
        finally { setKey(""); setBusy(false); }
    }

    return <section aria-label="Experimental TypeSafe computer mode">
        <h4>Experimental: TypeSafe/Jev browser goals</h4>
        <p>Optional bounded Jev goal execution inside the computer MCP. When enabled, agents should prefer one browser goal over repeated single browser steps. Normal desktop tools are unchanged.</p>
        <p>When used, visible page text, URLs, labels, ordinary field values, your goal and exact supplied values, plus a compact executed-action trace are sent to TypeSafe. Do not use on pages containing secrets or sensitive data you have not authorized for sharing.</p>
        <label><input type="checkbox" checked={enabled} disabled={!ready || busy} onChange={event => void toggle(event.target.checked)} /> Enable TypeSafe browser mode</label>
        {enabled && <div>
            <p>Computer MCP enablement, browser attachment and normal session permissions are still required. Goals have explicit step/time budgets and stop on uncertainty or consequential-action risk. DOM pages only: no screenshot vision, native desktop control, background automation, passwords or file uploads.</p>
            <label>TypeSafe API key<input type="password" autoComplete="off" spellCheck={false} value={key} disabled={busy} onChange={event => setKey(event.target.value)} /></label>
            <button disabled={busy || !key.trim()} onClick={() => void saveKey()}>Save TypeSafe key</button>
            <button disabled={busy} onClick={() => void removeKey()}>Remove saved TypeSafe key</button>
            <p>Alternatively set TYPESAFE_API_KEY in the agent server environment. Stored keys are never loaded into this form.</p>
        </div>}
        {notice && <p role="status">{notice}</p>}
    </section>;
}
