import { describe, expect, it, vi } from 'vitest';
import { browserServerAddress, nativeEntry, NativeServerRegistry, savedServer, serverAddress } from './serverRegistry';
const native = { id: 'entry', name: 'Native', endpoint: 'wss://host.example/prefix/session', agent_api: false, directory: '' };
describe('native registry protocol and address compatibility', () => {
    it('round-trips native sockets and proxy prefixes without changing the host or inventing TLS', () => {
        expect(browserServerAddress('ws://host.example:7981/session', 'daemon')).toBe('http://host.example:7981');
        expect(serverAddress(native.endpoint, 'daemon')).toBe('https://host.example/prefix');
        expect(nativeEntry(savedServer(native))).toEqual(native);
        expect(() => serverAddress('ws://host.example:7981/session', 'daemon')).toThrow('HTTPS');
        expect(() => serverAddress('ws://localhost:7981/session', 'daemon', 'https://gui.example')).toThrow('HTTPS');
        expect(serverAddress('ws://localhost:7981/session', 'daemon', 'http://localhost')).toBe('http://localhost:7981');
    });
    it('refuses Unix sockets and secrets in URLs, and never drops unsupported native entries from the list', () => {
        for (const url of ['unix:///run/neoism.sock', 'wss://user:password@host/session', 'https://host?token=secret', 'https://host/#secret'])
            expect(() => serverAddress(url, 'daemon')).toThrow();
        const unsupported = savedServer({ ...native, endpoint: 'unix:///run/neoism.sock' });
        expect(unsupported.address).toBe('unix:///run/neoism.sock');
    });
    it('requires the authenticated capability envelope instead of treating old backends as an empty list', async () => {
        for (const body of [{ servers: [] }, [], { capability: 'other', scope: 'daemon-os-user', servers: [] }]) {
            const bridge = new NativeServerRegistry(vi.fn().mockResolvedValue(new Response(JSON.stringify(body))));
            await expect(bridge.list()).rejects.toThrow('Saved servers unavailable');
        }
        const bridge = new NativeServerRegistry(vi.fn().mockResolvedValue(new Response('<html>old GUI</html>')));
        await expect(bridge.list()).rejects.toThrow('Saved servers unavailable');
    });
    it('only serializes approved fields in row compare-and-swap and never exports native passwords', async () => {
        const fetched = { ...native, password: 'must-not-reexport', hosted: { state_dir: '/private' } };
        const row = savedServer(fetched);
        const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ entry: native })));
        await new NativeServerRegistry(fetcher).save({ ...row, name: 'New name' }, row);
        const [, options] = fetcher.mock.calls[0];
        expect(JSON.parse(options.body).expected).toEqual(native);
        expect(options.body).not.toMatch(/must-not-reexport|private|operator-secret/);
        expect(options.headers.Authorization).toBeUndefined(); expect(options.credentials).toBe('same-origin'); expect(options.redirect).toBe('error');
    });
    it('does not treat guest authorization failure as an empty registry', async () => {
        const bridge = new NativeServerRegistry(vi.fn().mockResolvedValue(new Response('{}', { status: 401 })));
        await expect(bridge.list()).rejects.toThrow('Saved servers unavailable');
    });
});
