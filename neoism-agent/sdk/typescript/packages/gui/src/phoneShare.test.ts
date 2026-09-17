import { describe, expect, it, vi } from 'vitest';
import { DaemonConnection, agentGuiOrigin, consumePairingBootstrap, daemonUrl } from './serverConnections';
import { bootstrapPhonePairing, localOperatorShareAvailable, operatorShareTarget } from './phoneShare';

describe('phone share pairing', () => {
    it('allows Tailscale HTTP origins and rejects mixed-content LAN HTTP', () => {
        expect(daemonUrl('http://100.64.0.7:7878', 'http://100.64.0.7:7878/agent-gui/')).toBe('http://100.64.0.7:7878');
        expect(() => daemonUrl('http://192.168.1.20:7878')).toThrow('HTTPS');
        expect(() => daemonUrl('https://host?token=secret')).toThrow();
    });
    it('maps local operator GUI to the daemon share endpoint without 4096 and hides phone QR', () => {
        expect(operatorShareTarget('http://127.0.0.1:4096', 'http://127.0.0.1:5174/')).toBe('http://127.0.0.1:7878');
        expect(operatorShareTarget('http://127.0.0.1:4096', 'http://127.0.0.1:4096/')).toBe('http://127.0.0.1:7878');
        expect(operatorShareTarget('https://remote.example/agent/workspaces/ws-1', 'http://127.0.0.1:5174/')).toBe('http://127.0.0.1:7878');
        (globalThis as { __NEOISM_DAEMON_HTTP__?: string }).__NEOISM_DAEMON_HTTP__ = 'http://127.0.0.1:36883';
        expect(operatorShareTarget('http://127.0.0.1:34155', 'http://127.0.0.1:34155/')).toBe('http://127.0.0.1:36883');
        delete (globalThis as { __NEOISM_DAEMON_HTTP__?: string }).__NEOISM_DAEMON_HTTP__;
        expect(operatorShareTarget('https://host/agent/workspaces/ws-1', 'https://gui.example/')).toBeUndefined();
        expect(localOperatorShareAvailable('https://host/agent/workspaces/ws-1', 'https://gui.example/')).toBe(false);
        expect(operatorShareTarget('http://127.0.0.1:4096', 'http://100.64.0.7:7878/agent-gui/')).toBeUndefined();
        expect(localOperatorShareAvailable('http://100.64.0.7:7878/agent/workspaces/ws-1', 'http://100.64.0.7:7878/agent-gui/')).toBe(false);
        expect(operatorShareTarget('http://127.0.0.1:4096', 'https://evil.example/')).toBeUndefined();
    });
    it('strips pair query and never treats tokens as pairing codes', () => {
        const location = new URL('http://100.64.0.7:7878/agent-gui/?pair=ABCD2345&workspace=ws-1&session=chat-9');
        const history = { replaceState: (_a: unknown, _b: unknown, url: URL) => { location.href = url.href; } };
        const win = { location, history, __NEOISM_PAIR__: 'ABCD2345' } as unknown as Window;
        expect(agentGuiOrigin(location.href)).toBe('http://100.64.0.7:7878');
        expect(consumePairingBootstrap(win)).toEqual({ pair: 'ABCD2345', workspace: 'ws-1', session: 'chat-9' });
        expect(location.search).toBe('');
        expect(consumePairingBootstrap({ location: new URL('http://x/agent-gui/?pair=token=secret'), history, __NEOISM_PAIR__: 'token=secret' } as unknown as Window)).toEqual({});
    });
    it('claims pairing then joins with session on the committed server, not the previous prefs', async () => {
        const fetcher = vi.fn()
            .mockResolvedValueOnce(new Response(JSON.stringify({ status: 'granted', device_token: 'device-secret' })))
            .mockResolvedValueOnce(new Response(JSON.stringify({ workspaces: [{ id: 'ws-1', title: 'Shared' }] })))
            .mockResolvedValueOnce(new Response('[]'));
        const join = vi.fn();
        const location = new URL('http://100.64.0.7:7878/agent-gui/?pair=ABCD2345&workspace=ws-1&session=chat-9');
        vi.stubGlobal('window', { location, history: { replaceState: (_a: unknown, _b: unknown, url: URL) => { location.href = url.href; } } });
        vi.stubGlobal('location', location);
        await expect(bootstrapPhonePairing({ name: 'Pixel', join, fetcher })).resolves.toBe(true);
        expect(JSON.parse(fetcher.mock.calls[0][1].body).requested_permissions).toEqual([]);
        expect(join).toHaveBeenCalledWith('http://100.64.0.7:7878/agent/workspaces/ws-1', 'device-secret', '', 'chat-9');
        expect(join.mock.calls[0][0]).not.toContain('4096');
        expect(location.href).not.toContain('device-secret');
        expect(location.search).toBe('');
        vi.unstubAllGlobals();
    });
    it('share requests never include tokens and reject credentialed URLs', async () => {
        const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
            status: 'ready', hint: 'scan', url: 'http://100.64.0.7:7878/agent-gui/?pair=ABCD2345&workspace=ws-1', qr_svg: '<svg></svg>', shared: true,
        })));
        const result = await new DaemonConnection('http://127.0.0.1:7878', fetcher).sharePhone({ workspaceId: 'ws-1', sessionId: 'chat' });
        expect(fetcher.mock.calls[0][0]).toBe('http://127.0.0.1:7878/agent-gui/share');
        expect(fetcher.mock.calls[0][1].headers.Authorization).toBeUndefined();
        expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ workspace_id: 'ws-1', session_id: 'chat', share_workspace: false });
        expect(result.url).not.toMatch(/token|bearer/i);
        await expect(new DaemonConnection('http://127.0.0.1:7878', vi.fn().mockResolvedValue(new Response(JSON.stringify({
            status: 'ready', hint: 'bad', url: 'http://x/?token=secret', shared: true,
        })))).sharePhone({ workspaceId: 'ws-1' })).rejects.toThrow('credentials');
    });
});
