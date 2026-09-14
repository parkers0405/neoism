export interface SharedWorkspace { id: string; title: string }
export interface DiscoveredServer { name: string; url: string }

/** No credentials in URLs, redirects, cookies, localStorage, or discovery requests. */
export function daemonUrl(value: string, page = globalThis.location?.href): string {
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash)
        throw new Error('Use an HTTP(S) server URL without credentials, query, or fragment.');
    const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
    if (url.protocol === 'http:' && (!loopback || (page && new URL(page).protocol === 'https:')))
        throw new Error('Use HTTPS for remote servers. An HTTPS GUI cannot join an HTTP daemon; configure an HTTPS same-origin gateway.');
    return url.href.replace(/\/+$/, '');
}
export function workspaceAgentUrl(base: string, id: string): string {
    if (!id || id === '.' || id === '..' || /[\\/]/.test(id)) throw new Error('Invalid workspace ID.');
    return `${daemonUrl(base)}/agent/workspaces/${encodeURIComponent(id)}`;
}
export function joinedDaemon(server: string): string | undefined {
    return server.match(/^(.*)\/agent\/workspaces\/[^/]+\/?$/)?.[1];
}
export function scopedAgentFetch(server: string, fetcher: typeof fetch = fetch): typeof fetch {
    return (input, init) => {
        // Validate even restored preferences before transmitting a remembered credential.
        const base = daemonUrl(server);
        const target = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
        if (!target.href.startsWith(base + '/')) return Promise.reject(new Error('Request escaped the selected workspace.'));
        return fetcher(input, { ...init, credentials: 'omit', redirect: 'error', cache: 'no-store' });
    };
}

export class DaemonConnection {
    readonly base: string;
    constructor(base: string, private readonly fetcher: typeof fetch = fetch) { this.base = daemonUrl(base); }
    private async request<T>(path: string, token = '', body?: unknown, signal?: AbortSignal): Promise<T> {
        let response: Response;
        try {
            response = await this.fetcher(this.base + path, {
                method: body === undefined ? 'GET' : 'POST', credentials: 'omit', redirect: 'error', cache: 'no-store', signal,
                headers: { Accept: 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
                ...(body === undefined ? {} : { body: JSON.stringify(body) }),
            });
        } catch (error) {
            if (signal?.aborted) throw error;
            throw new Error('Cannot reach the daemon. Check HTTPS and network access. Cross-origin joining requires a gateway allowing this GUI origin, Authorization, Content-Type, and streaming responses.');
        }
        if (!response.ok) throw new Error(response.status === 401 || response.status === 403
            ? 'Daemon authorization rejected. Pair again or check your daemon credential.'
            : `Daemon request failed (${response.status}). Ensure this server supports agent-workspaces discovery.`);
        return response.json() as Promise<T>;
    }
    async pair(code: string, label: string, signal?: AbortSignal): Promise<string> {
        const result = await this.request<{ status: string; device_token?: string }>('/pair/claim', '', {
            code: code.trim(), device_label: label || 'Neoism GUI', requested_permissions: [],
        }, signal);
        if (result.status !== 'granted' || !result.device_token)
            throw new Error(result.status === 'pending' ? 'Awaiting host approval. Retry pairing after approval.' : 'Pairing rejected or expired. Request a new code from the host.');
        return result.device_token;
    }
    async workspaces(token: string, signal?: AbortSignal): Promise<SharedWorkspace[]> {
        if (!token.trim()) throw new Error('Pair with the host or enter a daemon credential first.');
        const result = await this.request<{ workspaces: SharedWorkspace[] }>('/agent-workspaces', token, undefined, signal);
        if (!Array.isArray(result.workspaces) || result.workspaces.some(w => typeof w.id !== 'string' || typeof w.title !== 'string')) throw new Error('Invalid workspace discovery response.');
        return result.workspaces;
    }
    async verify(id: string, token: string, signal?: AbortSignal): Promise<void> {
        workspaceAgentUrl(this.base, id);
        await this.request(`/agent/workspaces/${encodeURIComponent(id)}/v2/capabilities`, token, undefined, signal);
    }
    async discover(signal?: AbortSignal): Promise<DiscoveredServer[]> {
        const [hosts, tailnet] = await Promise.all([
            this.request<{ name: string; base_url: string }[]>('/hosts', '', undefined, signal),
            this.request<{ peers: { hostname: string; ip: string; online: boolean }[] }>('/tailnet-peers', '', undefined, signal),
        ]);
        return [...hosts.map(h => ({ name: h.name, url: h.base_url })),
            ...tailnet.peers.filter(p => p.online).map(p => ({ name: p.hostname, url: `http://${p.ip.includes(':') ? `[${p.ip}]` : p.ip}:7878` }))];
    }
}
