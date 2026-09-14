// @vitest-environment happy-dom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ServerConnections } from './ServerConnections';
import { ProviderConnections } from './ProviderConnections';
import { defaultPreferences } from '../types';
import { credentialKey, localServer, nativeEntry, savedServer, serverCredentials, type NativeServer, type SavedServer } from '../serverRegistry';
import type { NeoismClient } from '@neoism/sdk';
let container: HTMLDivElement, root: Root;
let nativeRows: NativeServer[], registryCalls: { method: string; body: any; headers: any }[], registryOverride: Response | undefined;
let agentFetch = vi.fn<(input: unknown, options?: RequestInit) => Promise<Response>>();
const result = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const team: SavedServer = { id: '00000000-0000-4000-8000-000000000001', name: 'Team', address: 'https://team.example', kind: 'daemon', directory: '' };
const seed = (...rows: SavedServer[]) => { nativeRows = rows.map(nativeEntry); };
beforeEach(() => {
    // Explicit storage per test. Do not depend on a worker's window/global alias,
    // or on controller tests' fake localStorage surviving in another suite.
    const data = new Map<string, string>();
    vi.stubGlobal('localStorage', { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => { data.set(k, v); }, removeItem: (k: string) => data.delete(k), clear: () => data.clear() });
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    serverCredentials.clear();
    nativeRows = []; registryCalls = []; registryOverride = undefined; agentFetch = vi.fn();
    vi.stubGlobal('fetch', vi.fn(async (url: URL | string, options: RequestInit = {}) => {
        if (String(url) !== '/__neoism/gui/servers') return agentFetch(url, options);
        const call = { method: options.method || 'GET', body: options.body ? JSON.parse(String(options.body)) : undefined, headers: options.headers as any }; registryCalls.push(call);
        if (registryOverride) return registryOverride.clone();
        expect(call.headers.Authorization).toBeUndefined();
        expect(call.headers['X-Neoism-Gui']).toBe('1');
        expect(options.credentials).toBe('same-origin');
        if (call.method === 'GET') return result({ capability: 'neoism.operator.server-registry', scope: 'daemon-os-user', servers: nativeRows });
        if (call.method === 'POST') {
            const existing = nativeRows.find(r => r.id === call.body.entry.id);
            if (JSON.stringify(existing || null) !== JSON.stringify(call.body.expected)) return result({}, 409);
            nativeRows = [...nativeRows.filter(r => r.id !== call.body.entry.id), call.body.entry]; return result({ entry: call.body.entry });
        }
        if (call.method === 'DELETE') { nativeRows = nativeRows.filter(r => r.id !== call.body.id); return result({ removed: true }); }
        return result({}, 400);
    }));
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.useRealTimers(); });
async function input(label: string, value: string) {
    const element = [...container.querySelectorAll('label')].find(e => e.textContent?.startsWith(label))!.querySelector('input,select')!;
    await act(async () => {
        const prototype = element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(prototype, 'value')!.set!.call(element, value);
        element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? 'change' : 'input', { bubbles: true }));
    });
}
async function click(text: string) {
    const button = [...container.querySelectorAll('button')].find(b => b.textContent?.trim() === text || b.getAttribute('aria-label') === text);
    expect(button, `button ${text}`).toBeTruthy(); await act(async () => button!.click());
}
async function select(name: string) { await act(async () => { [...container.querySelectorAll<HTMLButtonElement>('.server-select')].find(b => b.querySelector('.server-name')?.textContent === name)!.click(); }); }
async function render(props: Partial<Parameters<typeof ServerConnections>[0]> = {}) {
    const join = props.join || vi.fn(); await act(async () => root.render(<ServerConnections value={defaultPreferences} token="" join={join} {...props} />)); return join;
}
async function add(address = 'wss://team.example/session', name = 'Team', kind = 'daemon') {
    await click('Add server'); await input('Server address', address); await input('Server name', name);
    if (kind !== 'daemon') await input('Connection', kind);
    await click('Add server');
}
describe('native shared registry server list', () => {
    it('loads native saved rows, preserves local, and shows selection/truthful status without plumbing', async () => {
        seed(team); await render({ connected: true });
        expect(container.querySelector('input[placeholder="Search servers..."]')).toBeTruthy();
        expect(container.querySelector('.server-row[data-selected="true"]')?.textContent).toContain(localServer().name);
        expect(container.querySelector('[aria-label="Connected"]')).toBeTruthy(); expect(container.querySelector('[aria-label="Not connected"]')).toBeTruthy();
        expect(container.querySelectorAll('form,details,input[type="password"]')).toHaveLength(0);
        expect(container.textContent).not.toMatch(/Discover|Advanced|Pairing code|Daemon credential|Save settings/);
        expect(container.textContent).toContain('wss://team.example/session');
        const search = container.querySelector('input')!;
        await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(search, 'team.example'); search.dispatchEvent(new Event('input', { bubbles: true })); });
        expect(container.querySelectorAll('.server-row')).toHaveLength(1);
    });
    it('adds to native registry without connecting, then pairs and chooses scope without leaking operator auth', async () => {
        agentFetch.mockResolvedValueOnce(result({ status: 'granted', device_token: 'paired-secret' }))
            .mockResolvedValueOnce(result({ workspaces: [{ id: 'w1', title: 'Project A' }, { id: 'w2', title: 'Project B' }] })).mockResolvedValueOnce(result([]));
        const join = await render({ token: 'local-secret' }); await add();
        expect(agentFetch).not.toHaveBeenCalled(); expect(nativeRows[0]).toMatchObject({ name: 'Team', endpoint: 'wss://team.example/session' });
        await select('Team'); await input('Pairing code', 'ABCDEF'); await click('Connect');
        expect(join).not.toHaveBeenCalled(); await click('Project B');
        expect(join).toHaveBeenCalledWith('https://team.example/agent/workspaces/w2', 'paired-secret', '');
        expect(JSON.stringify(agentFetch.mock.calls)).not.toMatch(/operator-secret|local-secret/);
        expect(JSON.stringify(registryCalls.map(c => c.body))).not.toMatch(/paired-secret|ABCDEF|local-secret/);
        expect(localStorage.getItem('neoism.gui.servers')).toBeNull();
        expect(localStorage.getItem('neoism.gui.server-registry-bridge')).toBeNull();
    });
    it('automatically verifies and joins a sole workspace', async () => {
        seed(team); serverCredentials.set(credentialKey(team), 'device');
        agentFetch.mockResolvedValueOnce(result({ workspaces: [{ id: 'w', title: 'Only' }] })).mockResolvedValueOnce(result([]));
        const join = await render(); await select('Team'); expect(join).toHaveBeenCalledWith('https://team.example/agent/workspaces/w', 'device', '');
    });
    it('cancels stale discovery without a late join', async () => {
        let resolve!: (response: Response) => void; agentFetch.mockImplementation(() => new Promise<Response>(r => { resolve = r; }));
        seed(team); serverCredentials.set(credentialKey(team), 'device'); const join = await render(); await select('Team'); await click('Cancel');
        expect(agentFetch.mock.calls[0][1]?.signal?.aborted).toBe(true);
        await act(async () => resolve(result({ workspaces: [{ id: 'old', title: 'Old' }] })));
        expect(container.querySelector('.server-list')).toBeTruthy(); expect(join).not.toHaveBeenCalled();
    });
    it('retains a one-use pairing grant after discovery errors and does not claim a join', async () => {
        seed(team); agentFetch.mockResolvedValueOnce(result({ status: 'granted', device_token: 'device' })).mockRejectedValueOnce(new TypeError('Failed to fetch'));
        const join = await render(); await select('Team'); await input('Pairing code', 'CODE'); await click('Connect');
        expect(container.querySelector('[role="alert"]')?.textContent).toContain('gateway'); expect(serverCredentials.get(credentialKey(team))).toBe('device'); expect(join).not.toHaveBeenCalled();
        await click('Back to servers'); expect(container.querySelector('[aria-label="Connection failed"]')).toBeTruthy();
    });
    it('never joins on scoped authorization failure', async () => {
        seed(team); serverCredentials.set(credentialKey(team), 'revoked');
        agentFetch.mockResolvedValueOnce(result({ workspaces: [{ id: 'w', title: 'Team' }] })).mockResolvedValueOnce(result({}, 403));
        const join = await render(); await select('Team'); expect(join).not.toHaveBeenCalled(); expect(container.textContent).toContain('authorization rejected');
    });
    it('edits native rows with an expected snapshot, clears endpoint auth and restores local', async () => {
        seed(team); const before = nativeRows[0]; const forget = vi.fn();
        const join = await render({ value: { ...defaultPreferences, server: `${team.address}/agent/workspaces/w` }, token: 'old-secret', forget, credentialFor: () => 'local-credential' });
        await click('Edit Team'); await input('Server address', 'wss://replacement.example/session'); await click('Save server');
        expect(registryCalls.find(c => c.method === 'POST')?.body.expected).toEqual(before);
        expect(forget).toHaveBeenCalledWith(team.address); expect(serverCredentials.has(credentialKey(team))).toBe(false);
        expect(nativeRows[0].endpoint).toBe('wss://replacement.example/session'); expect(join).toHaveBeenCalledWith(localServer().address, 'local-credential', ''); expect(agentFetch).not.toHaveBeenCalled();
    });
    it('rejects stale desktop edits without clobbering them and refreshes on returning to the list', async () => {
        seed(team); await render(); await click('Edit Team'); await input('Server name', 'Web name');
        nativeRows[0] = { ...nativeRows[0], name: 'Desktop name' }; await click('Save server');
        expect(container.querySelector('[role="alert"]')?.textContent).toContain('changed in Neoism'); expect(nativeRows[0].name).toBe('Desktop name');
        await click('Back to servers'); await act(async () => window.dispatchEvent(new Event('focus')));
        expect(container.textContent).toContain('Desktop name');
    });
    it('removes active native entries only after confirmation and keeps server chats intact', async () => {
        seed(team); const forget = vi.fn();
        const join = await render({ value: { ...defaultPreferences, server: `${team.address}/agent/workspaces/w` }, token: 'device', forget, credentialFor: () => 'local-credential' });
        await click('Remove Team'); expect(container.textContent).toContain('Chats on the server will not be deleted'); await click('Cancel'); expect(nativeRows).toHaveLength(1);
        await click('Remove Team'); await click('Remove server'); expect(nativeRows).toEqual([]); expect(forget).toHaveBeenCalledWith(team.address); expect(join).toHaveBeenCalledWith(localServer().address, 'local-credential', '');
    });
    it('reflects desktop additions and removals on focus without a duplicate browser registry', async () => {
        await render(); seed(team); await act(async () => window.dispatchEvent(new Event('focus'))); expect(container.textContent).toContain('Team');
        nativeRows = []; await act(async () => window.dispatchEvent(new Event('focus'))); expect(container.textContent).not.toContain('Team'); expect(localStorage.getItem('neoism.gui.servers')).toBeNull();
    });
    it('uses the real default Agent endpoint without requiring registry or pairing access', async () => {
        registryOverride = result({}, 404); agentFetch.mockResolvedValue(result({ healthy: true })); const join = await render({ token: 'local-token' });
        expect(container.querySelector('[aria-label="Connected"]')).toBeNull(); await select(localServer().name);
        expect(String(agentFetch.mock.calls[0][0])).toBe(localServer().address + '/v2/health'); expect(join).toHaveBeenCalledWith(localServer().address, 'local-token', '');
    });
    it('keeps direct Agent API/directory support in the same Add/Edit flow with contextual auth', async () => {
        agentFetch.mockResolvedValueOnce(result({ message: 'Authentication required' }, 401)).mockResolvedValueOnce(result({ healthy: true })); const join = await render();
        await click('Add server'); await input('Server address', 'https://agent.example/api'); await input('Server name', 'Direct'); await input('Connection', 'agent'); await input('Workspace directory', '/project'); await click('Add server');
        await select('Direct'); expect(container.textContent).toContain('Agent bearer token'); expect(container.textContent).not.toContain('Pairing code'); await input('Agent bearer token', 'agent-secret'); await click('Connect');
        expect(join).toHaveBeenCalledWith('https://agent.example/api', 'agent-secret', '/project'); expect(nativeRows[0].agent_api).toBe(true); expect(JSON.stringify(nativeRows)).not.toContain('agent-secret');
    });
    it('keeps insecure/unsupported native rows visible but never sends credentials over plaintext or Unix', async () => {
        nativeRows = [{ ...nativeEntry(team), endpoint: 'ws://remote.example:7878/session' }]; await render(); await select('Team'); expect(container.querySelector('[role="alert"]')?.textContent).toContain('HTTPS'); expect(agentFetch).not.toHaveBeenCalled();
        await click('Edit Team'); await input('Server address', 'unix:///tmp/neoism.sock'); await click('Save server'); expect(container.querySelector('[role="alert"]')?.textContent).toContain('Unix sockets');
    });
    it('reports old/disabled backends honestly instead of showing a fake empty saved list', async () => {
        registryOverride = result({}, 404); await render(); expect(container.textContent).toContain('Saved servers unavailable in this session.'); expect(container.textContent).not.toContain('Connect local registry');
        expect(container.querySelector('.server-name')?.textContent).toBe(localServer().name); expect(container.textContent).not.toContain('No saved servers');
    });
    it('automatically loads same-origin saved entries while chat is remote, without sending guest or operator credentials', async () => {
        seed(team);
        await render({ value: { ...defaultPreferences, server: 'https://guest.example/agent/workspaces/w' }, token: 'guest-secret' });
        expect(registryCalls[0].headers.Authorization).toBeUndefined();
        expect(container.querySelector('[aria-label="Edit Team"]')).toBeTruthy();
        expect(JSON.stringify(registryCalls)).not.toContain('guest-secret');
        expect(container.textContent).not.toMatch(/Connect local registry|Operator credential|Local daemon address/);
        expect(container.querySelector('input[type="password"]')).toBeNull();
    });
    it('gates guest provider flows before requesting accounts or auth methods', async () => {
        const list = vi.fn().mockResolvedValue([{ id: 'neoism.providers.manage', enabled: false }]); const providers = { list: vi.fn(), authMethods: vi.fn() };
        const client = { capabilities: { list }, catalog: { providers } } as unknown as NeoismClient;
        await act(async () => root.render(<ProviderConnections shared client={client} directory="" initialProviderId="openai" />));
        expect(container.textContent).toContain('Ask the host'); expect(providers.list).not.toHaveBeenCalled(); expect(providers.authMethods).not.toHaveBeenCalled();
    });
});
