import { describe, expect, it, vi } from 'vitest';
import { Readable } from 'node:stream';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { localRegistryHandler } from '../scripts/localRegistryProxy';
const PRIVATE = 'a'.repeat(64);
function client(path = '/', overrides: Record<string, string | undefined> = {}, body = '', peer = '127.0.0.1') {
    const req = Readable.from(body ? [body] : []) as unknown as IncomingMessage;
    Object.assign(req, { url: path, method: body ? 'POST' : 'GET', socket: { remoteAddress: peer }, headers: {
        host: '127.0.0.1:5174', 'sec-fetch-site': path === '/' ? 'none' : 'same-origin', 'sec-fetch-mode': path === '/' ? 'navigate' : 'cors', 'sec-fetch-dest': path === '/' ? 'document' : 'empty', 'x-neoism-gui': '1', ...overrides,
    } });
    const headers: Record<string, string> = {}; let bodyOut = '';
    const res = { statusCode: 200, setHeader: (key: string, value: string) => { headers[key.toLowerCase()] = value; }, end: (body = '') => { bodyOut = body; } };
    return { req, res: res as unknown as ServerResponse, headers, body: () => bodyOut, next: vi.fn() };
}
function setup(target = 'http://127.0.0.1:4096', agentToken?: string) {
    const upstream = vi.fn(async (url: URL, _method: string, _headers: Record<string, string>, _body?: string) => url.pathname === '/'
        ? new Response('public GUI', { headers: { 'x-neoism-agent-gui': '1', 'set-cookie': `neoism_local_gui=${PRIVATE}; HttpOnly; SameSite=Strict; Path=/` } })
        : new Response(JSON.stringify({ capability: 'neoism.operator.server-registry', scope: 'daemon-os-user', servers: [{ id: 'native', name: 'Native saved', endpoint: 'ws://localhost:7878/session' }] }), { headers: { 'content-type': 'application/json' } }));
    const handler = localRegistryHandler({ target, port: () => 5174, agentToken, upstream });
    const run = async (call: ReturnType<typeof client>) => { await handler(call.req, call.res, call.next); return call; };
    const launch = async () => { const nav = await run(client()); return nav.headers['set-cookie'].split(';')[0]; };
    return { upstream, run, launch };
}
describe('automatic local Vite registry proxy', () => {
    it('bootstraps server-side on local navigation and shares native entries without exposing upstream auth', async () => {
        const { upstream, run, launch } = setup();
        const cookie = await launch(); expect(upstream).not.toHaveBeenCalled();
        const response = await run(client('/__neoism/gui/servers', { cookie }));
        expect(response.res.statusCode).toBe(200); expect(response.body()).toContain('Native saved');
        expect(upstream.mock.calls[0][2]['sec-fetch-site']).toBe('none');
        expect(upstream.mock.calls[1][2].cookie).toBe(`neoism_local_gui=${PRIVATE}`);
        expect(upstream.mock.calls[1][2].authorization).toBeUndefined();
        expect(response.headers['set-cookie']).toBeUndefined(); expect(response.body()).not.toContain(PRIVATE); expect(cookie).not.toContain(PRIVATE);
        expect(response.headers['access-control-allow-origin']).toBeUndefined();
    });
    it('reuses configured local API auth exclusively server-side, never as a browser registry credential', async () => {
        const { upstream, run, launch } = setup('http://127.0.0.1:4096', 'existing-api-key');
        const response = await run(client('/__neoism/gui/servers', { cookie: await launch() }));
        expect(response.res.statusCode).toBe(200);
        expect(upstream.mock.calls[0][2].authorization).toBe('Bearer existing-api-key');
        expect(upstream.mock.calls[1][2].authorization).toBeUndefined();
        expect(response.body()).not.toContain('existing-api-key');
        expect(response.headers['set-cookie']).toBeUndefined();
    });
    it('denies cross-site/iframe launches, guest bearers and missing launch sessions', async () => {
        const { run, upstream } = setup();
        for (const headers of [{ 'sec-fetch-site': 'cross-site' }, { 'sec-fetch-dest': 'iframe' }, { host: 'evil.example:5174' }, { authorization: 'Bearer guest' }]) {
            const response = await run(client('/', headers)); expect(response.headers['set-cookie']).toBeUndefined();
        }
        const noSession = await run(client('/__neoism/gui/servers')); expect(noSession.res.statusCode).toBe(403); expect(upstream).not.toHaveBeenCalled();
    });
    it('rejects cross-origin CSRF, same-site different port, forwarded and remote requests even with a cookie', async () => {
        const { run, launch, upstream } = setup(); const cookie = await launch();
        for (const headers of [{ origin: 'https://evil.example' }, { 'sec-fetch-site': 'same-site' }, { host: 'localhost:9000' }, { forwarded: 'host=evil' }, { 'x-neoism-gui': '' }, { authorization: 'Bearer guest' }]) {
            expect((await run(client('/__neoism/gui/servers', { cookie, ...headers }))).res.statusCode).toBe(403);
        }
        expect((await run(client('/__neoism/gui/servers', { cookie }, '', '100.1.2.3'))).res.statusCode).toBe(403); expect(upstream).not.toHaveBeenCalled();
    });
    it('never bootstraps a remote target or follows a redirect/old backend response', async () => {
        const remote = setup('https://remote.example'); const cookie = await remote.launch();
        expect((await remote.run(client('/__neoism/gui/servers', { cookie }))).res.statusCode).toBe(404); expect(remote.upstream).not.toHaveBeenCalled();
        const old = setup(); old.upstream.mockResolvedValue(new Response('', { status: 302, headers: { location: 'https://remote.example' } }));
        expect((await old.run(client('/__neoism/gui/servers', { cookie: await old.launch() }))).res.statusCode).toBe(404); expect(old.upstream).toHaveBeenCalledTimes(1);
    });
    it('passes targeted writes through without browser auth/cookies or operator secrets', async () => {
        const { run, launch, upstream } = setup(); const cookie = await launch();
        const body = JSON.stringify({ entry: { id: 'native', name: 'New name' }, expected: { id: 'native', name: 'Old name' } });
        await run(client('/__neoism/gui/servers', { cookie: cookie + '; unrelated=browser-secret' }, body));
        expect(upstream.mock.calls[1][1]).toBe('POST'); expect(upstream.mock.calls[1][3]).toBe(body); expect(upstream.mock.calls[1][2].cookie).not.toContain('browser-secret');
    });
});
