// @vitest-environment happy-dom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, it, expect, vi } from 'vitest';
import type { MessageWithParts } from '@neoism/sdk';
import { Picker } from './components/Picker';
import { Timeline } from './components/Timeline';
import { MemoTimeline } from './components/MemoTimeline';
import * as runtime from './components/runtimeMessages';
import { responseMetadata } from './components/responseMetadata';
import { filterChoices, indexChoices, layoutChoices, visibleChoices, revealChoice } from './pickerState';
import { groupedModelChoices, rememberModel, saveRecentModels, loadRecentModels } from './modelRecents';
import { messageUsage, deferredPersistence } from './controllerPerformance';

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
afterEach(() => vi.unstubAllGlobals());
const noop = () => {};
const choices = Array.from({ length: 10000 }, (_, i) => ({ id: `provider-${Math.floor(i / 100)}/model-${i}`, label: `Model ${i}`, description: `Provider ${Math.floor(i / 100)}`, section: `Provider ${Math.floor(i / 100)}` }));
const history = Array.from({ length: 500 }, (_, i) => ({ info: { id: `m${String(i).padStart(4, '0')}`, sessionId: 's', role: i % 2 ? 'assistant' : 'user', agent: 'build', modelId: 'model', providerId: 'provider', time: { created: i * 1000, completed: i * 1000 + 500 } }, parts: [{ id: `p${i}`, type: 'text', text: ('A representative history paragraph with **emphasis** and a [link](https://example.com).\n\n').repeat(4) }] })) as unknown as MessageWithParts[];

describe('bounded grouped picker', () => {
    it('preserves case-insensitive multiword AND matches and excludes empty sections', () => {
        const matches = filterChoices(indexChoices(choices), 'PROVIDER 17 model-1701');
        expect(matches.map(c => c.id)).toEqual(['provider-17/model-1701']);
        const html = renderToStaticMarkup(<Picker title="Models" choices={matches} choose={noop} close={noop} />);
        expect(html).toContain('choice-section'); expect(html).not.toContain('Provider 18');
    });
    it('keeps Current then eight MRU refs then duplicate provider rows like native', () => {
        let recent: string[] = [];
        for (let i = 0; i < 10; i++) recent = rememberModel(recent, choices[i].id);
        recent = rememberModel(recent, choices[5].id);
        expect(recent).toHaveLength(8); expect(recent[0]).toBe(choices[5].id);
        const storage = new Map<string, string>();
        vi.stubGlobal('localStorage', { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) });
        saveRecentModels('https://a', recent);
        expect(loadRecentModels('https://a')).toEqual(recent); expect(loadRecentModels('https://b')).toEqual([]);
        const grouped = groupedModelChoices(choices, recent, choices[5].id);
        expect(grouped[0].section).toBe('Current'); expect(grouped[1].section).toBe('Recent');
        expect(grouped.filter(c => c.id === choices[5].id)).toHaveLength(3);
        expect(groupedModelChoices([], [], 'missing/model')[0].badge).toContain('unavailable');
    });
    it('mounts at most eight selectable rows and arrows/Tab scroll across the continuous list', async () => {
        const host = document.createElement('div'); document.body.append(host); const root = createRoot(host); const choose = vi.fn();
        try {
            await act(async () => root.render(<Picker title="Models" choices={choices} choose={choose} close={noop} />));
            const input = host.querySelector('input')!;
            for (let i = 0; i < 9; i++) await act(async () => { input.dispatchEvent(new KeyboardEvent('keydown', { key: i === 8 ? 'Tab' : 'ArrowDown', bubbles: true })); });
            expect(host.querySelectorAll('[role=option]').length).toBeLessThanOrEqual(8);
            expect(host.querySelector('[aria-selected=true]')?.getAttribute('aria-posinset')).toBe('10');
            await act(async () => { input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); });
            expect(choose).toHaveBeenLastCalledWith(choices[9].id);
            expect(host.textContent).not.toContain('Previous'); expect(host.textContent).not.toContain('Next');
            const layout = layoutChoices(choices), top = revealChoice(layout, 9999, 0);
            expect(visibleChoices(layout, top).filter(r => r.choice).length).toBeLessThanOrEqual(8);
            const list = host.querySelector('[role=listbox]')! as HTMLDivElement;
            await act(async () => { list.scrollTop = layout.options[5000].top; list.dispatchEvent(new Event('scroll', { bubbles: true })); });
            expect(host.querySelector('[role=option]')?.getAttribute('aria-posinset')).toBe('5001');
            expect(host.querySelectorAll('[role=option]').length).toBeLessThanOrEqual(8);
            // Scrolling never snaps back to the offscreen keyboard selection.
            expect(list.scrollTop).toBe(layout.options[5000].top);
        } finally { await act(async () => root.unmount()); host.remove(); }
    });
    it('shows real pending choices, not No matches', () => {
        const html = renderToStaticMarkup(<Picker title="Models" choices={[]} loading choose={noop} close={noop} />);
        expect(html).toContain('skeleton'); expect(html).not.toContain('No matches');
    });
});

describe('history render boundary and Node profiles', () => {
    it('omits all ordinary user headings without guessing identity or changing bodies', () => {
        const messages = ['You', 'parkersettle', 'Alex', undefined].map((author, i) => ({ ...history[0], info: { ...history[0].info, id: `user${i}`, author }, parts: [{ id: `p${i}`, type: 'text', text: 'parkersettle says heywhatsup' }] })) as unknown as MessageWithParts[];
        const html = renderToStaticMarkup(<Timeline messages={messages} busy={false} loading={false} loadOlder={noop} />);
        expect(html).not.toContain('message-label');
        expect(html).not.toContain('remote-user'); expect(html).toContain('parkersettle says heywhatsup');
        expect(renderToStaticMarkup(<Timeline messages={[]} busy={false} loading loadOlder={noop} />)).toContain('skeleton-conversation');
    });
    it('profiles actual Node rendering and proves unrelated updates skip all 500 history normalizations', async () => {
        const measure = (fn: () => unknown) => { const t = performance.now(); fn(); return performance.now() - t; };
        const legacyPicker = () => renderToStaticMarkup(<div>{choices.map(c => <button key={c.id} role="option"><strong>{c.label}</strong><small>{c.description}</small></button>)}</div>);
        const boundedPicker = () => renderToStaticMarkup(<Picker title="Models" choices={choices} choose={noop} close={noop} />);
        legacyPicker(); boundedPicker();
        const legacyMs = measure(legacyPicker), boundedMs = measure(boundedPicker);
        const projectionMs = measure(() => { runtime.normalizeMessages(history); responseMetadata(history); messageUsage(history); });
        const host = document.createElement('div'); document.body.append(host); const root = createRoot(host);
        const spy = vi.spyOn(runtime, 'normalizeMessages');
        const props = { messages: history, busy: false, loading: false, loadOlder: noop };
        try {
            await act(async () => root.render(<Timeline {...props} />)); spy.mockClear();
            let t = performance.now();
            // Force old full-history work by invalidating every row identity. This
            // remains a reproducible baseline after row memoization lands.
            for (let i = 0; i < 5; i++) await act(async () => root.render(<Timeline {...props} messages={history.map(m => ({ ...m }))} />));
            const beforeMs = performance.now() - t; expect(spy).toHaveBeenCalledTimes(2500);
            await act(async () => root.render(<MemoTimeline {...props} />)); spy.mockClear();
            t = performance.now();
            for (let i = 0; i < 5; i++) await act(async () => root.render(<MemoTimeline {...props} />));
            const afterMs = performance.now() - t; expect(spy).not.toHaveBeenCalled();
            await act(async () => root.render(<MemoTimeline {...props} messages={[...history]} />)); expect(spy).not.toHaveBeenCalled();
            const streamed = history.map((m, i) => i === history.length - 1 ? { ...m, parts: m.parts.map(p => p.type === 'text' ? { ...p, text: p.text + ' delta' } : p) } : m);
            t = performance.now();
            await act(async () => root.render(<MemoTimeline {...props} messages={streamed} />));
            const deltaMs = performance.now() - t;
            expect(spy).toHaveBeenCalledTimes(1); expect(spy.mock.calls[0][0]).toHaveLength(1);
            console.log(JSON.stringify({ fixture: { models: choices.length, messages: history.length, textBytes: JSON.stringify(history).length }, pickerSSR: { beforeMs: legacyMs, afterMs: boundedMs, optionNodesBefore: choices.length, optionNodesAfter: (boundedPicker().match(/role="option"/g) || []).length }, historyProjectionMs: projectionMs, fiveUnrelatedUpdates: { beforeMs, afterMs, normalizedRowsBefore: 2500, normalizedRowsAfter: 0 }, streamedDelta: { deltaMs, normalizedRows: 1 } }));
        } finally { spy.mockRestore(); await act(async () => root.unmount()); host.remove(); }
    }, 20000);
    it('flushes pending scope writes on lifecycle without retaining history in persistence', () => {
        const write = vi.fn(); const p = deferredPersistence(write);
        for (let i = 0; i < 100; i++) p.schedule('a', i);
        p.schedule('b', 2); expect(write).not.toHaveBeenCalled(); p.flush();
        expect(write.mock.calls).toEqual([['a', 99], ['b', 2]]);
    });
});
