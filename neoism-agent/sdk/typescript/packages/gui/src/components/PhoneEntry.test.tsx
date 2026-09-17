// @vitest-environment happy-dom
import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { PhoneEntry } from './PhoneEntry';
import { PhoneShareModal } from './PhoneShare';
const mocks = vi.hoisted(() => ({ pair: vi.fn(), share: vi.fn() }));
vi.mock('../phoneShare', () => ({ bootstrapPhonePairing: mocks.pair, requestPhoneShare: mocks.share }));
vi.mock('./Modal', () => ({ Modal: ({ children }: any) => <div>{children}</div> }));
let root: Root, node: HTMLDivElement;
beforeEach(() => {
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal('location', new URL('http://100.64.0.7:7878/agent-gui/'));
    mocks.pair.mockReset(); mocks.share.mockReset();
    node = document.createElement('div'); document.body.append(node); root = createRoot(node);
});
afterEach(() => { act(() => root.unmount()); node.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
it('claims once before mounting any chat client, including StrictMode effect replay', async () => {
    let finish!: () => void;
    mocks.pair.mockImplementation(input => new Promise(resolve => { finish = () => {
        input.join('http://100.64.0.7:7878/agent/workspaces/ws-1', 'secret', '', 'chat-9'); resolve(true);
    }; }));
    const children = vi.fn(connection => <span>{connection.server}</span>);
    await act(async () => { root.render(<StrictMode><PhoneEntry>{children}</PhoneEntry></StrictMode>); });
    expect(children).not.toHaveBeenCalled();
    expect(mocks.pair).toHaveBeenCalledOnce();
    await act(async () => finish());
    expect(children).toHaveBeenCalledWith({ server: 'http://100.64.0.7:7878/agent/workspaces/ws-1', token: 'secret', sessionId: 'chat-9' });
    expect(node.textContent).not.toContain('secret');
});
it('does not start an unauthenticated chat when a code has expired', async () => {
    mocks.pair.mockRejectedValue(new Error('expired'));
    const children = vi.fn();
    await act(async () => { root.render(<PhoneEntry>{children}</PhoneEntry>); });
    expect(children).not.toHaveBeenCalled();
    expect(node.textContent).toBe('Scan a new QR code to connect.');
});
it('shows a clean QR and renews automatically without sharing again', async () => {
    vi.useFakeTimers(); vi.setSystemTime(100000);
    mocks.share.mockImplementation(async () => ({ status: 'ready', hint: 'Scan on a phone on the same Tailscale network.',
        expires_at: Math.floor(Date.now() / 1000) + 60, qr_svg: '<svg></svg>', shared: true }));
    await act(async () => { root.render(<PhoneShareModal server="http://127.0.0.1:4096" workspaceId="ws-1" directory="/work" close={() => {}} />); });
    expect(node.textContent).toBe('Done');
    expect(node.querySelector('[aria-label="QR code for phone pairing"]')).not.toBeNull();
    await act(async () => { vi.advanceTimersByTime(60200); });
    expect(mocks.share).toHaveBeenCalledTimes(2);
    expect(mocks.share.mock.calls[1][0].shareWorkspace).toBe(false);
    expect(node.textContent).toBe('Done');
    act(() => root.render(<div />));
    await act(async () => { vi.advanceTimersByTime(120000); });
    expect(mocks.share).toHaveBeenCalledTimes(2);
});
