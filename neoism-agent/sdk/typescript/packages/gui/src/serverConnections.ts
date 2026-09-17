export interface LocalWorkspace { id: string; title: string; directory: string; shared: boolean }
export interface SharedWorkspace { id: string; title: string }
export interface DiscoveredServer { name: string; url: string }
export interface AgentShareResult {
    status: string;
    url?: string;
    hint: string;
    expires_at?: number;
    workspace_id?: string;
    shared: boolean;
    qr_svg?: string;
}

const LOOPBACK = ['localhost', '127.0.0.1', '[::1]'];
const CGNAT = /^100\.(6[4-9]|[7-9]\d|1[0-1]\d|12[0-7])\./;

function tailnetHttpAllowed(hostname: string) {
    return hostname.endsWith('.ts.net') || CGNAT.test(hostname);
}

/** No credentials in URLs, redirects, cookies, localStorage, or discovery requests. */
export function daemonUrl(value: string, page = globalThis.location?.href): string {
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash)
        throw new Error('Use an HTTP(S) server URL without credentials, query, or fragment.');
    const loopback = LOOPBACK.includes(url.hostname);
    const tailnet = tailnetHttpAllowed(url.hostname);
    const pageHttps = !!(page && new URL(page).protocol === 'https:');
    if (url.protocol === 'http:' && (pageHttps || (!loopback && !tailnet)))
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
export function joinedWorkspace(server: string): string | undefined {
    return server.match(/\/agent\/workspaces\/([^/]+)\/?$/)?.[1];
}
export function agentGuiOrigin(href = globalThis.location?.href): string | undefined {
    try {
        const url = new URL(href);
        if (!url.pathname.startsWith('/agent-gui')) return;
        url.pathname = '/'; url.search = ''; url.hash = '';
        return daemonUrl(url.href, href);
    } catch { return; }
}
type PairingWindow = { location: { href: string }; history?: { replaceState(a: unknown, b: string, url: URL | string): void }; __NEOISM_PAIR__?: unknown };
export function consumePairingBootstrap(win: PairingWindow = globalThis.window): { pair?: string; workspace?: string; session?: string } {
    const injected = typeof win.__NEOISM_PAIR__ === 'string' ? String(win.__NEOISM_PAIR__) : '';
    try { delete win.__NEOISM_PAIR__; } catch { /* Visit-local only. */ }
    const page = new URL(win.location.href);
    const pair = injected || page.searchParams.get('pair') || '';
    const workspace = page.searchParams.get('workspace') || '';
    const session = page.searchParams.get('session') || '';
    if (page.search) {
        page.search = '';
        try { win.history?.replaceState(null, '', page); } catch { /* Private browsing. */ }
    }
    if (/token|bearer|device/i.test(pair) || pair.includes('/') || pair.length > 16) return {};
    return { pair: pair || undefined, workspace: workspace || undefined, session: session || undefined };
}
export function scopedAgentFetch(server: string, fetcher: typeof fetch = fetch): typeof fetch {
    return (input, init) => {
        // Validate even restored preferences before transmitting a remembered credential.
        const base = daemonUrl(server);
        const target = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
        if (!target.href.startsWith(base + '/')) return Promise.reject(new Error('Request escaped the selected workspace.'));
        // An empty chooser value means the proxy's workspace root, not a request
        // for an empty directory that overrides the daemon's trusted header.
        if (target.searchParams.get('directory') === '') {
            target.searchParams.delete('directory');
            input = typeof input === 'string' || input instanceof URL ? target.href : new Request(target, input);
        }
        return fetcher(input, { ...init, credentials: 'omit', redirect: 'error', cache: 'no-store' });
    };
}

export class DaemonConnection {
    readonly base: string;
    constructor(base: string, private readonly fetcher: typeof fetch = fetch) { this.base = daemonUrl(base); }
    private async request<T>(path: string, token = '', body?: unknown, signal?: AbortSignal): Promise<T> {
        let response: Response;
        try {
            const fetcher = this.fetcher;
            response = await fetcher(this.base + path, {
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
            : response.status === 429
            ? 'Too many pairing requests. Wait a moment and try again.'
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
    async localWorkspaces(signal?: AbortSignal): Promise<LocalWorkspace[]> {
        const result = await this.request<{ workspaces: LocalWorkspace[] }>('/agent-gui/workspaces', '', {}, signal);
        if (!Array.isArray(result.workspaces) || result.workspaces.some(row =>
            !row || typeof row.id !== 'string' || typeof row.title !== 'string' || typeof row.directory !== 'string'))
            throw new Error('Invalid workspace list from the Neoism daemon.');
        return result.workspaces;
    }
    async sharePhone(input: { workspaceId?: string; sessionId?: string; directory?: string; shareWorkspace?: boolean }, signal?: AbortSignal): Promise<AgentShareResult> {
        const result = await this.request<AgentShareResult>('/agent-gui/share', '', {
            workspace_id: input.workspaceId || undefined,
            session_id: input.sessionId || undefined,
            directory: input.directory || undefined,
            share_workspace: !!input.shareWorkspace,
        }, signal);
        if (typeof result.status !== 'string' || typeof result.hint !== 'string') throw new Error('Invalid share response.');
        if (result.url && (/token=|bearer|device_token/i.test(result.url) || result.url.includes('#'))) throw new Error('Share URL must not include credentials.');
        return result;
    }
}
