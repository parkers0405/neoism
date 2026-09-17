import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Plus, Search } from 'lucide-react';
import { createHttpTransport } from '@neoism/sdk';
import { DaemonConnection, scopedAgentFetch, type SharedWorkspace, workspaceAgentUrl } from '../serverConnections';
import { credentialKey, currentServer, localServer, matchesServer, serverAddress, serverCredentials, NativeServerRegistry, type SavedServer } from '../serverRegistry';
import type { Preferences } from '../types';
import './server-connections.css';

type Step = { type: 'edit'; row: SavedServer; fresh: boolean } | { type: 'auth' | 'workspaces'; row: SavedServer } | { type: 'remove'; row: SavedServer };
export function ServerConnections({ value, token, connected = false, join, forget, credentialFor, initialAdd = false, registryChanged, joined }: {
    value: Preferences; token: string; connected?: boolean;
    join(server: string, token: string, directory?: string): void;
    forget?(server: string): void;
    credentialFor?(server: string): string;
    initialAdd?: boolean;
    registryChanged?(): void;
    joined?(): void;
}) {
    const [rows, setRows] = useState<SavedServer[]>([]);
    const [registryReady, setRegistryReady] = useState(false);
    const [registryError, setRegistryError] = useState('');
    const [loadingRegistry, setLoadingRegistry] = useState(true);
    const registryRequest = useRef<AbortController | null>(null);
    const bridge = useRef<NativeServerRegistry | null>(null);
    const [query, setQuery] = useState('');
    const [step, setStep] = useState<Step | undefined>(() => initialAdd ? { type: 'edit', fresh: true,
        row: { id: crypto.randomUUID(), name: '', address: '', kind: 'daemon', directory: '' } } : undefined);
    const [authMethod, setAuthMethod] = useState<'pair' | 'token'>('pair');
    const [secret, setSecret] = useState('');
    const [workspaces, setWorkspaces] = useState<SharedWorkspace[]>([]);
    const [busy, setBusy] = useState<string>();
    const [error, setError] = useState('');
    const [failed, setFailed] = useState<string>();
    const request = useRef<AbortController | null>(null);
    const active = currentServer(value);
    const activeRows = [localServer(), ...rows, ...(active && !rows.some(row => matchesServer(row, value)) ? [active] : [])];
    const current = activeRows.find(row => matchesServer(row, value));
    const listRef = useRef<HTMLInputElement>(null);
    useEffect(() => {
        if (current && token) serverCredentials.set(credentialKey(current), token);
    }, [current?.address, current?.kind, token]);
    const rowsRef = useRef(rows); rowsRef.current = rows;
    const live = useRef({ value, join, forget, credentialFor }); live.current = { value, join, forget, credentialFor };
    const loadRegistry = async () => {
        registryRequest.current?.abort();
        const controller = new AbortController(); registryRequest.current = controller;
        setLoadingRegistry(!registryReady); setRegistryError('');
        const timer = setTimeout(() => {
            if (!controller.signal.aborted && registryRequest.current === controller) { controller.abort(); setLoadingRegistry(false); setRegistryError('Saved servers unavailable in this session.'); }
        }, 15000);
        controller.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
        try {
            const client = new NativeServerRegistry();
            const found = await client.list(controller.signal);
            if (controller.signal.aborted) return;
            const previous = rowsRef.current.find(row => matchesServer(row, live.current.value));
            if (previous && !found.some(row => row.id === previous.id && row.address === previous.address && row.kind === previous.kind && row.directory === previous.directory)) {
                serverCredentials.delete(credentialKey(previous)); live.current.forget?.(previous.address);
                const local = localServer(); live.current.join(local.address, serverCredentials.get(credentialKey(local)) || live.current.credentialFor?.(local.address) || '', '');
            }
            bridge.current = client;
            setRows(found); setRegistryReady(true);
            return true;
        } catch (e) { if (!controller.signal.aborted) { setRegistryReady(false); setRegistryError(e instanceof Error ? e.message : 'Native registry unavailable.'); } }
        finally { clearTimeout(timer); if (registryRequest.current === controller) setLoadingRegistry(false); }
    };
    useEffect(() => {
        void loadRegistry();
        return () => { request.current?.abort(); registryRequest.current?.abort(); };
    }, []);
    useEffect(() => {
        if (step || !registryReady) return;
        const refresh = () => { void loadRegistry(); };
        const timer = setInterval(refresh, 5000);
        window.addEventListener('focus', refresh);
        return () => { clearInterval(timer); window.removeEventListener('focus', refresh); };
    }, [step, registryReady]);
    // External switches invalidate a pending connect before it can change the new selection.
    useEffect(() => { request.current?.abort(); setBusy(undefined); setStep(undefined); setSecret(''); }, [value.server, token]);
    const cancel = () => {
        registryRequest.current?.abort(); setLoadingRegistry(false);
        request.current?.abort(); request.current = null;
        setBusy(undefined); setStep(undefined); setSecret(''); setWorkspaces([]); setError('');
        queueMicrotask(() => listRef.current?.focus());
    };
    const run = async (row: SavedServer, action: (signal: AbortSignal) => Promise<void>) => {
        request.current?.abort();
        const controller = new AbortController(); request.current = controller;
        setBusy(row.id); setFailed(undefined); setError('');
        const timeout = setTimeout(() => {
            if (request.current === controller) {
                controller.abort(); setBusy(undefined); setFailed(row.id); setError('Connection timed out. Check the server address, HTTPS, and gateway access.');
            }
        }, 20000);
        try { await action(controller.signal); }
        catch (e) { if (!controller.signal.aborted) { setFailed(row.id); setError(e instanceof Error ? e.message : 'Connection failed.'); } }
        finally { clearTimeout(timeout); if (request.current === controller) setBusy(undefined); }
    };
    const finish = (row: SavedServer, credential: string, server: string) => {
        serverCredentials.set(credentialKey(row), credential);
        setStep(undefined); setSecret('');
        join(server, credential, row.kind === 'agent' ? row.directory : '');
        joined?.();
    };
    const verifyWorkspace = async (row: SavedServer, id: string, credential: string, signal: AbortSignal) => {
        await new DaemonConnection(row.address).verify(id, credential, signal);
        if (!signal.aborted) finish(row, credential, workspaceAgentUrl(row.address, id));
    };
    const loadWorkspaces = async (row: SavedServer, credential: string, signal: AbortSignal) => {
        const found = await new DaemonConnection(row.address).workspaces(credential, signal);
        if (signal.aborted) return;
        serverCredentials.set(credentialKey(row), credential);
        setSecret(''); setWorkspaces(found); setStep({ type: 'workspaces', row });
        if (found.length === 1) await verifyWorkspace(row, found[0].id, credential, signal);
    };
    const verifyAgent = async (row: SavedServer, credential: string, signal: AbortSignal) => {
        const baseUrl = serverAddress(row.address, 'agent');
        try {
            await createHttpTransport({ baseUrl, token: credential || undefined, fetch: scopedAgentFetch(baseUrl) })
                .request({ path: '/v2/health', signal });
        } catch (e) {
            if (!signal.aborted && (e as { status?: number }).status === 401) { setStep({ type: 'auth', row }); setAuthMethod('token'); }
            if (e instanceof TypeError) throw new Error('Cannot reach the server. Check HTTPS and network access; cross-origin requests require a gateway allowing this GUI origin and authenticated requests.');
            throw e;
        }
        if (!signal.aborted) finish(row, credential, baseUrl);
    };
    const connect = (row: SavedServer) => {
        cancel();
        try { serverAddress(row.address, row.kind); } catch (e) { setFailed(row.id); setError(e instanceof Error ? e.message : 'Unsupported server address.'); return; }
        const credential = matchesServer(row, value) ? token : serverCredentials.get(credentialKey(row)) || credentialFor?.(row.address) || '';
        if (row.kind === 'daemon') {
            setStep({ type: 'auth', row }); setAuthMethod(credential ? 'token' : 'pair');
            if (credential) void run(row, signal => loadWorkspaces(row, credential, signal));
        } else {
            setStep({ type: 'auth', row }); setAuthMethod('token');
            void run(row, signal => verifyAgent(row, credential, signal));
        }
    };
    const forgetRow = (row: SavedServer) => {
        serverCredentials.delete(credentialKey(row)); forget?.(row.address);
    };
    const fallback = () => {
        const local = localServer();
        join(local.address, serverCredentials.get(credentialKey(local)) || credentialFor?.(local.address) || '', local.directory);
    };
    const update = (row: SavedServer) => {
        void run(row, async signal => {
            if (!bridge.current || !registryReady) throw new Error('Connect the native registry before saving servers.');
            const next = { ...row, id: row.id === 'current' ? crypto.randomUUID() : row.id, address: serverAddress(row.address, row.kind), name: row.name.trim() || new URL(row.address).host, directory: row.kind === 'agent' ? row.directory : '' };
            if (rows.some(other => other.id !== row.id && other.address === next.address && other.kind === next.kind)) throw new Error('That server is already saved.');
            const previous = rows.find(r => r.id === row.id);
            const saved = await bridge.current.save(next, previous, signal);
            if (signal.aborted) return;
            const changedEndpoint = previous && (previous.address !== saved.address || previous.kind !== saved.kind || previous.directory !== saved.directory);
            setRows(previous ? rows.map(r => r.id === row.id ? saved : r) : [...rows, saved]); cancel();
            registryChanged?.();
            if (changedEndpoint) { forgetRow(previous); if (matchesServer(previous, value)) fallback(); }
        });
    };
    const remove = (row: SavedServer) => {
        void run(row, async signal => {
            if (!bridge.current || !registryReady) throw new Error('Connect the native registry before removing servers.');
            await bridge.current.remove(row, signal);
            if (signal.aborted) return;
            setRows(rows.filter(r => r.id !== row.id)); cancel(); forgetRow(row);
            if (matchesServer(row, value)) fallback();
        });
    };
    const beginEdit = (row?: SavedServer) => {
        cancel();
        setStep({ type: 'edit', fresh: !row, row: row || { id: crypto.randomUUID(), name: '', address: '', kind: 'daemon', directory: '' } });
    };
    const visible = activeRows.filter(row => `${row.name} ${row.address}`.toLowerCase().includes(query.trim().toLowerCase()));
    return <section className="server-picker" aria-label="Servers" onKeyDown={event => {
        if (event.key === 'Escape' && step) { event.preventDefault(); event.stopPropagation(); cancel(); }
    }}>
        {!step ? <>
            <label className="server-search"><Search size={16} aria-hidden="true" /><input ref={listRef} type="search" placeholder="Search servers..." aria-label="Search servers" value={query} onChange={e => setQuery(e.target.value)} /></label>
            <div className="server-list">
                {visible.map(row => {
                    const selected = matchesServer(row, value);
                    const status = busy === row.id ? 'Connecting' : failed === row.id ? 'Connection failed' : selected && connected ? 'Connected' : selected ? 'Selected — not verified' : 'Not connected';
                    return <div key={row.id} className="server-row" data-selected={selected}>
                        <button type="button" className="server-select" aria-pressed={selected} onClick={() => connect(row)}>
                            <span className="server-dot" data-status={status} role="img" aria-label={status} title={status} />
                            <span className="server-name">{row.name}</span><span className="server-address">{row.native?.endpoint || row.address}</span>
                        </button>
                        {row.id !== 'local' && <div className="server-actions">
                            <button type="button" aria-label={`Edit ${row.name}`} onClick={() => beginEdit(row)}>Edit</button>
                            {row.native && <button type="button" disabled={!registryReady} aria-label={`Remove ${row.name}`} onClick={() => { cancel(); setStep({ type: 'remove', row }); }}>Remove</button>}
                        </div>}
                    </div>;
                })}
                {!visible.length && <p role="status">No matching servers.</p>}
            </div>
            {loadingRegistry && <p role="status">Loading Neoism’s saved servers…</p>}
            {registryError && <p className="settings-note" role="status">{registryError}</p>}
            <button type="button" className="server-add" onClick={() => beginEdit()}><Plus size={16} aria-hidden="true" /> Add server</button>
        </> : <>
            <button type="button" className="server-back" onClick={cancel}><ArrowLeft size={16} aria-hidden="true" /> Back to servers</button>
            {step.type === 'edit' ? <form className="server-form" onSubmit={event => { event.preventDefault(); update(step.row); }}>
                <fieldset className="server-fields" disabled={!!busy}>
                <h3>{step.fresh ? 'Add server' : 'Edit server'}</h3>
                <label>Server address<input autoFocus required placeholder="wss://workstation.example/session" value={step.row.address} onChange={e => { setError(''); setStep({ ...step, row: { ...step.row, address: e.target.value } }); }} /></label>
                <label>Server name (optional)<input placeholder="Home workstation" value={step.row.name} onChange={e => setStep({ ...step, row: { ...step.row, name: e.target.value } })} /></label>
                <label>Connection<select value={step.row.kind} onChange={e => setStep({ ...step, row: { ...step.row, kind: e.target.value as SavedServer['kind'] } })}><option value="daemon">Neoism server</option><option value="agent">Agent API</option></select></label>
                {step.row.kind === 'agent' && <label>Workspace directory<input placeholder="Server default" value={step.row.directory} onChange={e => setStep({ ...step, row: { ...step.row, directory: e.target.value } })} /></label>}
                {!registryReady && <p role="status">Saved servers unavailable in this session. You can still connect without saving.</p>}
                <footer><button type="button" onClick={cancel}>Cancel</button>{!registryReady && <button type="button" onClick={() => connect(step.row)}>Connect without saving</button>}<button type="submit" disabled={!registryReady}>{step.fresh ? 'Add server' : 'Save server'}</button></footer>
                </fieldset>
            </form> : step.type === 'remove' ? <div className="server-form">
                <h3>Remove {step.row.name}?</h3>
                <p>{matchesServer(step.row, value) ? `This disconnects the selected server and returns to ${localServer().name}. ` : ''}Chats on the server will not be deleted.</p>
                <footer><button type="button" onClick={cancel}>Cancel</button><button type="button" onClick={() => remove(step.row)}>Remove server</button></footer>
            </div> : <div className="server-form">
                <h3>{step.type === 'workspaces' ? 'Choose workspace' : `Connect to ${step.row.name}`}</h3>
                <p className="server-address">{step.row.address}</p>
                {step.type === 'workspaces' ? <>
                    {workspaces.map(workspace => <button type="button" key={workspace.id} disabled={!!busy} onClick={() => void run(step.row, signal => verifyWorkspace(step.row, workspace.id, serverCredentials.get(credentialKey(step.row)) || '', signal))}>{workspace.title || workspace.id}</button>)}
                    {!workspaces.length && <p role="status">No shared workspaces. Ask the host to share one.</p>}
                    <button type="button" disabled={!!busy} onClick={() => connect(step.row)}>Retry connection</button>
                </> : !busy && <form onSubmit={event => {
                    event.preventDefault();
                    void run(step.row, async signal => {
                        if (step.row.kind === 'agent') return verifyAgent(step.row, secret, signal);
                        const daemon = new DaemonConnection(step.row.address);
                        const credential = authMethod === 'pair' ? await daemon.pair(secret, value.name, signal) : secret;
                        if (!signal.aborted) {
                            // Pairing codes are single-use. Retain the grant even if discovery fails.
                            serverCredentials.set(credentialKey(step.row), credential);
                            setAuthMethod('token'); setSecret(credential);
                            await loadWorkspaces(step.row, credential, signal);
                        }
                    });
                }}>
                    {step.row.kind === 'daemon' && <label>Connect with<select value={authMethod} onChange={e => { setAuthMethod(e.target.value as 'pair' | 'token'); setSecret(''); setError(''); }}><option value="pair">Pairing code</option><option value="token">Daemon credential</option></select></label>}
                    <label>{step.row.kind === 'agent' ? 'Agent bearer token' : authMethod === 'pair' ? 'Pairing code' : 'Daemon credential'}<input autoFocus type="password" autoComplete="off" value={secret} onChange={e => setSecret(e.target.value)} required={step.row.kind === 'daemon'} /></label>
                    <p className="settings-note">{step.row.kind === 'daemon' ? 'Use a host-issued pairing code or daemon credential. Native WebSocket-only passwords are not supported by this chat connection. ' : ''}Credentials stay in memory until this page is closed.</p>
                    <button type="submit">Connect</button>
                </form>}
                {busy && <p role="status">Connecting…</p>}
                <button type="button" onClick={cancel}>Cancel</button>
            </div>}
        </>}
        {error && <p className="server-error" role="alert">{error}</p>}
    </section>;
}
