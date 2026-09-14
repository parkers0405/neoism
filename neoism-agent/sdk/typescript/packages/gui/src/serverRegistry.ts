import { daemonUrl, joinedDaemon } from './serverConnections';
import { defaultPreferences, type Preferences } from './types';
export type ServerKind = 'daemon' | 'agent';
export interface NativeServer { id: string; name: string; endpoint: string; agent_api?: boolean; directory?: string }
export interface SavedServer { id: string; name: string; address: string; kind: ServerKind; directory: string; native?: NativeServer }

/** Normalize native socket addresses structurally for display; security is checked again before connecting. */
export function browserServerAddress(address: string, kind: ServerKind): string {
    const url = new URL(address.trim());
    if (url.protocol === 'unix:') throw new Error('Browsers cannot connect to Unix sockets. Enter the server’s HTTP(S) address.');
    if (kind === 'daemon') {
        if (url.protocol === 'ws:') url.protocol = 'http:';
        if (url.protocol === 'wss:') url.protocol = 'https:';
        if (url.pathname.replace(/\/+$/, '').endsWith('/session')) url.pathname = url.pathname.replace(/\/session\/*$/, '') || '/';
    }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash)
        throw new Error('Use a server address without URL credentials, query or fragment.');
    return url.href.replace(/\/+$/, '');
}
export function serverAddress(address: string, kind: ServerKind, page = globalThis.location?.href): string {
    return daemonUrl(browserServerAddress(address, kind), page);
}
export function localServer(): SavedServer {
    let local = false;
    try { local = ['localhost', '127.0.0.1', '[::1]'].includes(new URL(defaultPreferences.server).hostname); } catch { /* Report errors on connect. */ }
    return { id: 'local', name: local ? 'Local Server' : 'Default Server', address: defaultPreferences.server, kind: 'agent', directory: '' };
}
export function matchesServer(row: SavedServer, value: Preferences): boolean {
    const endpoint = row.kind === 'daemon' ? joinedDaemon(value.server) : value.server;
    return endpoint?.replace(/\/+$/, '') === row.address.replace(/\/+$/, '') && (row.kind === 'daemon' || row.directory === value.directory);
}
export function currentServer(value: Preferences): SavedServer | undefined {
    if (matchesServer(localServer(), value)) return;
    const daemon = joinedDaemon(value.server);
    try {
        const address = browserServerAddress(daemon || value.server, daemon ? 'daemon' : 'agent');
        return { id: 'current', name: new URL(address).host, address, kind: daemon ? 'daemon' : 'agent', directory: daemon ? '' : value.directory };
    } catch { return; }
}
export const serverCredentials = new Map<string, string>();
export const credentialKey = (row: SavedServer) => `${row.kind}:${row.address}`;

export function nativeEntry(row: SavedServer): NativeServer {
    const url = new URL(browserServerAddress(row.address, row.kind));
    if (row.kind === 'daemon') { url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'; url.pathname = url.pathname.replace(/\/+$/, '') + '/session'; }
    return { id: row.id, name: row.name, endpoint: url.href.replace(/\/+$/, ''), agent_api: row.kind === 'agent', directory: row.kind === 'agent' ? row.directory : '' };
}
export function savedServer(native: NativeServer): SavedServer {
    const kind = native.agent_api ? 'agent' : 'daemon';
    // Unsupported old/native addresses stay visible and editable, never silently disappear.
    let address = native.endpoint;
    try { address = browserServerAddress(native.endpoint, kind); } catch { /* Connect reports why this browser cannot use it. */ }
    return { id: native.id, name: native.name, address, kind, directory: native.directory || '', native: { id: native.id, name: native.name, endpoint: native.endpoint, agent_api: !!native.agent_api, directory: native.directory || '' } };
}
export class NativeServerRegistry {
    constructor(private readonly fetcher: typeof fetch = fetch) {}
    private async request<T>(method: string, body?: unknown, signal?: AbortSignal): Promise<T> {
        let response: Response;
        try {
            response = await this.fetcher('/__neoism/gui/servers', {
                method, headers: { Accept: 'application/json', 'X-Neoism-Gui': '1', ...(body ? { 'Content-Type': 'application/json' } : {}) },
                credentials: 'same-origin', redirect: 'error', cache: 'no-store', signal, ...(body ? { body: JSON.stringify(body) } : {}),
            });
        } catch (e) {
            if (signal?.aborted) throw e;
            throw new Error('Saved servers unavailable in this session.');
        }
        if ([401, 403, 404].includes(response.status)) throw new Error('Saved servers unavailable in this session.');
        if (response.status === 409) throw new Error('This server changed in Neoism. Go back and reload before editing again.');
        if (!response.ok) throw new Error('Could not update saved servers. Try again.');
        try { return await response.json() as T; } catch { throw new Error('Saved servers unavailable in this session.'); }
    }
    async list(signal?: AbortSignal): Promise<SavedServer[]> {
        const result = await this.request<{ capability: string; scope: string; servers: NativeServer[] }>('GET', undefined, signal);
        if (result.capability !== 'neoism.operator.server-registry' || result.scope !== 'daemon-os-user' || !Array.isArray(result.servers) || result.servers.some(s => typeof s.id !== 'string' || typeof s.name !== 'string' || typeof s.endpoint !== 'string'))
            throw new Error('Saved servers unavailable in this session.');
        return result.servers.map(savedServer);
    }
    async save(row: SavedServer, previous?: SavedServer, signal?: AbortSignal): Promise<SavedServer> {
        const result = await this.request<{ entry: NativeServer }>('POST', { entry: nativeEntry(row), expected: previous?.native || null }, signal);
        return savedServer(result.entry);
    }
    async remove(row: SavedServer, signal?: AbortSignal): Promise<void> {
        if (!row.native) throw new Error('This connection is not saved in the native registry.');
        await this.request('DELETE', row.native, signal);
    }
}
