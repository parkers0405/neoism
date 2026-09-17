import { DaemonConnection, agentGuiOrigin, consumePairingBootstrap, joinedDaemon, workspaceAgentUrl, type AgentShareResult } from './serverConnections';

export interface PhoneConnection { server: string; token: string; sessionId?: string }

const LOOPBACK = ['127.0.0.1', 'localhost', '[::1]'];

function loopbackHttp(url: URL) {
    return LOOPBACK.includes(url.hostname) && url.protocol === 'http:';
}

function hintedDaemon(page?: string): string | undefined {
    const hinted = (globalThis as { __NEOISM_DAEMON_HTTP__?: unknown }).__NEOISM_DAEMON_HTTP__;
    if (typeof hinted !== 'string') return;
    try {
        const url = new URL(hinted);
        if (!loopbackHttp(url) || url.username || url.password || url.search || url.hash) return;
        if (page) {
            const origin = new URL(page);
            if (!loopbackHttp(origin)) return;
        }
        return url.href.replace(/\/+$/, '');
    } catch { return; }
}

export function operatorShareTarget(server: string, page = globalThis.location?.href): string | undefined {
    if (!page || agentGuiOrigin(page)) return;
    try {
        const url = new URL(page);
        if (!loopbackHttp(url)) return;
        // Operator capabilities belong to the machine serving this GUI, not to
        // the currently selected chat server. A remote chat must not make the
        // local workspace switcher (or phone sharing) disappear.
        void server;
        const hint = hintedDaemon(page);
        const port = url.port || (url.protocol === 'https:' ? '443' : '80');
        if (!hint && !['5174', '4096', '7878'].includes(port)) return;
        return hint || (port === '7878' ? url.origin : 'http://127.0.0.1:7878');
    } catch { return; }
}

export async function requestPhoneShare(input: {
    server: string; workspaceId?: string; sessionId?: string; directory?: string; shareWorkspace?: boolean;
}, signal?: AbortSignal): Promise<AgentShareResult> {
    if (typeof (globalThis as { __NEOISM_DAEMON_HTTP__?: unknown }).__NEOISM_DAEMON_HTTP__ !== 'string') {
        try {
            const response = await fetch('/__neoism/gui/share-target', { credentials: 'same-origin', cache: 'no-store', signal, headers: { Accept: 'application/json' } });
            if (response.ok) {
                const body = await response.json() as { daemon?: string };
                if (typeof body.daemon === 'string') (globalThis as { __NEOISM_DAEMON_HTTP__?: string }).__NEOISM_DAEMON_HTTP__ = body.daemon;
            }
        } catch { /* Fall back to :7878. */ }
    }
    const target = operatorShareTarget(input.server);
    if (!target) throw new Error('Phone share is only available from the local operator GUI talking to this machine’s daemon.');
    return new DaemonConnection(target).sharePhone({
        workspaceId: input.workspaceId, sessionId: input.sessionId, directory: input.directory, shareWorkspace: input.shareWorkspace,
    }, signal);
}

export async function bootstrapPhonePairing(input: {
    name: string;
    join(server: string, token: string, directory?: string, sessionId?: string): void;
    fetcher?: typeof fetch;
}): Promise<boolean> {
    const origin = agentGuiOrigin();
    if (!origin) return false;
    const boot = consumePairingBootstrap();
    if (!boot.pair || !boot.workspace) return false;
    const daemon = new DaemonConnection(origin, input.fetcher);
    const token = await daemon.pair(boot.pair, input.name || 'Phone');
    const rows = await daemon.workspaces(token);
    if (!rows.some(row => row.id === boot.workspace)) throw new Error('That pairing code is not for this shared workspace.');
    await daemon.verify(boot.workspace, token);
    input.join(workspaceAgentUrl(origin, boot.workspace), token, '', boot.session);
    return true;
}

export function pendingPhonePair(href = globalThis.location?.href, injected?: unknown) {
    if (!agentGuiOrigin(href)) return false;
    try {
        const pair = (typeof injected === 'string' ? injected : '') || new URL(href).searchParams.get('pair') || '';
        return !!pair && pair.length <= 16 && !/token|bearer|device/i.test(pair) && !pair.includes('/');
    } catch { return false; }
}

export function localOperatorShareAvailable(server: string, page = globalThis.location?.href) {
    return !!operatorShareTarget(server, page);
}
