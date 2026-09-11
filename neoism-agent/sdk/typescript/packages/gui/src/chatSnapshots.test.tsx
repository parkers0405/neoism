// @vitest-environment happy-dom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, it, expect, vi } from 'vitest';
import type { NeoismClient, MessageWithParts } from '@neoism/sdk';
import { useChat } from './useChat';
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
const notify = vi.fn();
const row = (id: string) => ({ info: { id, sessionId: 'a', role: 'user', time: { created: 1 } }, parts: [] }) as unknown as MessageWithParts;
function client() {
    const request = vi.fn(async (operation: string, input: any) => operation === 'v2.sessions.messages'
        ? { items: [row(input.query.cursor ? 'older' : input.path.session_id)], cursor: { next: input.query.cursor ? undefined : 'older-cursor' } }
        : operation === 'v2.sessions.queue.list' ? { sessionId: input.path.session_id, count: 0, items: [], running: false, worker: false }
        : { sessionId: input.path.session_id, revision: 1, branches: [] });
    return { operations: { request }, sessions: { status: vi.fn(async () => ({})) }, events: { async *subscribe({ signal }: { signal: AbortSignal }) { await new Promise<void>(resolve => signal.addEventListener('abort', () => resolve(), { once: true })); } } } as unknown as NeoismClient;
}

describe('session-scoped history snapshots', () => {
    it('returns new-session empty/loading before effects and restores fresh cached older pages without requests', async () => {
        const host = document.createElement('div'); const root = createRoot(host); const api = client();
        let chat!: ReturnType<typeof useChat>; const renders: { id?: string; messages: string[]; loading: boolean }[] = [];
        function Host({ id, client = api }: { id?: string; client?: NeoismClient }) {
            chat = useChat(client, id, notify);
            renders.push({ id, messages: chat.state.messages.map(m => m.info.id), loading: chat.loading });
            return null;
        }
        try {
            await act(async () => root.render(<Host id="a" />));
            expect(chat.state.messages.map(m => m.info.id)).toEqual(['a']);
            await act(async () => chat.loadOlder());
            expect(chat.state.messages).toHaveLength(2);
            const before = vi.mocked(api.operations.request).mock.calls.filter(([op]) => op === 'v2.sessions.messages').length;
            renders.length = 0;
            await act(async () => root.render(<Host id="b" />));
            expect(renders[0]).toEqual({ id: 'b', messages: [], loading: true });
            renders.length = 0;
            await act(async () => root.render(<Host id="a" />));
            expect(renders[0].messages).toEqual(expect.arrayContaining(['a', 'older']));
            expect(renders[0].messages).toHaveLength(2);
            expect(vi.mocked(api.operations.request).mock.calls.filter(([op]) => op === 'v2.sessions.messages')).toHaveLength(before + 1);
            expect(chat.older).toBeUndefined();
            await act(async () => root.render(<Host id="b" />));
            const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 16000);
            try {
                await act(async () => root.render(<Host id="a" />));
                expect(vi.mocked(api.operations.request).mock.calls.filter(([op]) => op === 'v2.sessions.messages')).toHaveLength(before + 2);
            } finally { clock.mockRestore(); }
            renders.length = 0;
            await act(async () => root.render(<Host id="a" client={client()} />));
            expect(renders[0]).toEqual({ id: 'a', messages: [], loading: true });
        } finally { await act(async () => root.unmount()); }
    });
    it('keeps live SSE detail provenance through polling but resets it before rendering a cached return', async () => {
        const api = client(), queue: any[] = []; let wake = () => {};
        api.events.subscribe = (async function* ({ signal }: { signal: AbortSignal }) {
            const stop = () => wake(); signal.addEventListener('abort', stop);
            try { while (!signal.aborted) { if (queue.length) yield queue.shift(); else await new Promise<void>(resolve => { wake = resolve; }); } }
            finally { signal.removeEventListener('abort', stop); }
        }) as any;
        const tool = { id: 'tool', type: 'tool', messageId: 'a', sessionId: 'a', callId: 'c', tool: 'bash', state: { status: 'completed', output: 'old' } };
        const initial = { ...row('a'), parts: [tool] } as unknown as MessageWithParts;
        vi.mocked(api.operations.request).mockImplementation(async (op, input: any) => op === 'v2.sessions.messages' ? { items: input.path.session_id === 'a' ? [initial] : [row('b')], cursor: {} } : { sessionId: input.path.session_id, revision: 1, branches: [] } as any);
        const root = createRoot(document.createElement('div')); let chat!: ReturnType<typeof useChat>; let firstLiveSize = -1;
        function Host({ id }: { id: string }) { chat = useChat(api, id, notify); if (firstLiveSize < 0) firstLiveSize = chat.liveParts.size; return null; }
        try {
            await act(async () => root.render(<Host id="a" />)); expect(chat.liveParts.size).toBe(0);
            await act(async () => { queue.push({ id: 'e1', type: 'message.part.updated', data: { sessionID: 'a', part: { ...tool, state: { status: 'completed', output: 'live' } } } }); wake(); });
            expect(chat.liveParts.get('a')?.has('tool')).toBe(true);
            const identity = chat.liveParts.get('a');
            await act(async () => { window.dispatchEvent(new Event('focus')); });
            expect(chat.liveParts.get('a')).toBe(identity);
            await act(async () => root.render(<Host id="b" />));
            firstLiveSize = -1; await act(async () => root.render(<Host id="a" />));
            expect(firstLiveSize).toBe(0); expect(chat.state.messages[0].parts).toHaveLength(1);
        } finally { await act(async () => root.unmount()); }
    });
    it('targets delayed create completion at its session rather than poisoning the cached unsent draft', async () => {
        const api = client(), root = createRoot(document.createElement('div')); let chat!: ReturnType<typeof useChat>;
        function Host({ id }: { id?: string }) { chat = useChat(api, id, notify); return null; }
        try {
            await act(async () => root.render(<Host />)); const unsentSetter = chat.setState;
            await act(async () => root.render(<Host id="a" />));
            await act(async () => unsentSetter(s => ({ ...s, busy: true }), 'a'));
            expect(chat.state.busy).toBe(true);
            await act(async () => root.render(<Host />)); expect(chat.state.busy).toBe(false);
        } finally { await act(async () => root.unmount()); }
    });
    it('does not populate a new tab from an aborted old-tab response', async () => {
        const api = client(); let resolve!: (v: any) => void;
        const original = vi.mocked(api.operations.request).getMockImplementation()!;
        vi.mocked(api.operations.request).mockImplementation(((operation: string, input: any) => operation === 'v2.sessions.messages' && input.path.session_id === 'a'
            ? new Promise(r => { resolve = r; }) : original(operation as any, input)) as any);
        const root = createRoot(document.createElement('div')); let chat!: ReturnType<typeof useChat>;
        function Host({ id }: { id: string }) { chat = useChat(api, id, notify); return null; }
        try {
            await act(async () => root.render(<Host id="a" />)); expect(chat.loading).toBe(true);
            await act(async () => root.render(<Host id="b" />));
            await act(async () => resolve({ items: [row('late-a')], cursor: {} }));
            expect(chat.state.messages.map(m => m.info.id)).toEqual(['b']);
        } finally { await act(async () => root.unmount()); }
    });
});
