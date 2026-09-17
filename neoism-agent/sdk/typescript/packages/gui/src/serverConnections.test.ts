import { describe, expect, it, vi } from 'vitest';
import { DaemonConnection, daemonUrl, joinedDaemon, workspaceAgentUrl, scopedAgentFetch } from './serverConnections';
import { createHttpTransport } from '@neoism/sdk';
const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
describe('daemon shared chat connection', () => {
    it('uses the daemon workspace default for empty directories without rewriting explicit paths', async () => {
        const fetcher = vi.fn().mockResolvedValue(response({}));
        const base = 'https://host/agent/workspaces/ws-1';
        const scoped = scopedAgentFetch(base, fetcher);
        await scoped(base + '/v2/agents?directory=&scope=workspace');
        expect(String(fetcher.mock.calls[0][0])).toBe(base + '/v2/agents?scope=workspace');
        await scoped(base + '/v2/agents?directory=%2Fother');
        expect(String(fetcher.mock.calls[1][0])).toBe(base + '/v2/agents?directory=%2Fother');
    });
    it('calls fetch without binding the daemon client as its receiver', async () => {
        const fetcher = vi.fn(function (this: unknown) {
            if (this !== undefined) throw new TypeError('Illegal invocation');
            return Promise.resolve(response({ status: 'no_workspace', hint: 'Open a workspace.', shared: false }));
        });
        const result = await new DaemonConnection('http://127.0.0.1:7878', fetcher).sharePhone({});
        expect(result.status).toBe('no_workspace');
        expect(fetcher).toHaveBeenCalledOnce();
    });
    it('rejects insecure remote, mixed content, credential-bearing and non-HTTP URLs', () => {
        for (const url of ['http://peer:7878', 'https://user:secret@host', 'https://host?token=secret', 'https://host/#secret', 'file:///tmp'])
            expect(() => daemonUrl(url)).toThrow();
        expect(() => daemonUrl('http://localhost:7878', 'https://gui.example')).toThrow('HTTPS');
        expect(daemonUrl('http://127.0.0.1:7878/', 'http://localhost')).toBe('http://127.0.0.1:7878');
        expect(daemonUrl('http://100.64.0.7:7878', 'http://100.64.0.7:7878/agent-gui/')).toBe('http://100.64.0.7:7878');
    });
    it('pairs without requesting terminal or device administration permissions and verifies scoped access', async () => {
        const fetcher = vi.fn().mockResolvedValueOnce(response({ status: 'granted', device_token: 'device-secret' }))
            .mockResolvedValueOnce(response({ workspaces: [{ id: 'work space', title: 'Shared' }] }))
            .mockResolvedValueOnce(response([]));
        const daemon = new DaemonConnection('https://host/neoism', fetcher);
        const token = await daemon.pair(' code ', 'Laptop');
        const rows = await daemon.workspaces(token);
        await daemon.verify(rows[0].id, token);
        expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ code: 'code', device_label: 'Laptop', requested_permissions: [] });
        expect(fetcher.mock.calls[0][1].headers).not.toHaveProperty('Authorization');
        expect(fetcher.mock.calls[1][1].headers.Authorization).toBe('Bearer device-secret');
        expect(fetcher.mock.calls[2][0]).toBe('https://host/neoism/agent/workspaces/work%20space/v2/capabilities');
        for (const [url, options] of fetcher.mock.calls) {
            expect(url).not.toContain('secret'); expect(options.credentials).toBe('omit'); expect(options.redirect).toBe('error');
        }
        expect(joinedDaemon(workspaceAgentUrl(daemon.base, rows[0].id))).toBe(daemon.base);
        for (const id of ['', '..', 'a/b', 'a\\b']) expect(() => workspaceAgentUrl(daemon.base, id)).toThrow();
    });
    it('never treats pending, rejected, invalid grants or denied workspace lists as a join', async () => {
        for (const result of [{ status: 'pending' }, { status: 'rejected' }, { status: 'granted' }]) {
            const daemon = new DaemonConnection('https://host', vi.fn().mockResolvedValue(response(result)));
            await expect(daemon.pair('code', 'GUI')).rejects.toThrow();
        }
        const fetcher = vi.fn().mockResolvedValue(response({}, 401));
        const daemon = new DaemonConnection('https://host', fetcher);
        await expect(daemon.workspaces('')).rejects.toThrow('credential');
        expect(fetcher).not.toHaveBeenCalled();
        await expect(daemon.workspaces('revoked')).rejects.toThrow('authorization rejected');
    });
    it('discovery never sends a credential to another host', async () => {
        const fetcher = vi.fn().mockResolvedValueOnce(response([{ name: 'Other', base_url: 'https://other' }]))
            .mockResolvedValueOnce(response({ peers: [{ hostname: 'peer', ip: '100.1.2.3', online: true }] }));
        const servers = await new DaemonConnection('https://host', fetcher).discover();
        expect(servers).toHaveLength(2);
        for (const [url, options] of fetcher.mock.calls) {
            expect(url).toMatch(/^https:\/\/host\//); expect(options.headers).not.toHaveProperty('Authorization');
        }
    });
    it('keeps SDK chat requests and live SSE on the verified workspace proxy', async () => {
        const baseUrl = 'https://host/prefix/agent/workspaces/team';
        const event = { id: 'event-1', sequence: 1, type: 'session.updated', data: {} };
        const fetcher = vi.fn().mockResolvedValueOnce(response({ items: [] }))
            .mockResolvedValueOnce(new Response(`data: ${JSON.stringify(event)}\n\n`, { headers: { 'Content-Type': 'text/event-stream' } }));
        const guarded = scopedAgentFetch(baseUrl, fetcher);
        const transport = createHttpTransport({ baseUrl, token: 'daemon-secret', fetch: guarded });
        await transport.request({ path: '/v2/sessions' });
        for await (const received of transport.events({ sessionId: 'chat' })) { expect(received.id).toBe('event-1'); break; }
        expect(String(fetcher.mock.calls[0][0])).toBe(baseUrl + '/v2/sessions');
        expect(String(fetcher.mock.calls[1][0])).toBe(baseUrl + '/v2/events?sessionId=chat');
        for (const [, options] of fetcher.mock.calls) {
            expect(options.headers.authorization).toBe('Bearer daemon-secret');
            expect(options.redirect).toBe('error'); expect(options.credentials).toBe('omit');
        }
        await expect(guarded('https://host/prefix/agent/workspaces/other/v2/sessions')).rejects.toThrow('escaped');
        expect(fetcher).toHaveBeenCalledTimes(2);
    });
    it('explains CORS failures and passes cancellation to fetch', async () => {
        const fetcher = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
        const daemon = new DaemonConnection('https://host', fetcher);
        const controller = new AbortController();
        await expect(daemon.workspaces('secret', controller.signal)).rejects.toThrow('gateway');
        expect(fetcher.mock.calls[0][1].signal).toBe(controller.signal);
    });
});
