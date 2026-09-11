import { afterEach, describe, expect, it, vi } from 'vitest';
import { closeTab, loadTabs, localTab, saveTabs } from './tabs';
afterEach(() => vi.unstubAllGlobals());
describe('tab state helpers', () => {
    it('uses stable distinct identities before a session exists', () => {
        const first = localTab(), second = localTab(); expect(first.key).not.toBe(second.key); expect(first.sessionId).toBeUndefined();
    });
    it('closes adjacent right then left and always leaves one usable view', () => {
        const tabs = [localTab(), localTab(), localTab()];
        const right = closeTab({ tabs, active: tabs[1].key }, tabs[1].key); expect(right.active).toBe(tabs[2].key);
        const left = closeTab(right, tabs[2].key); expect(left.active).toBe(tabs[0].key);
        const last = closeTab(left, tabs[0].key); expect(last.tabs).toHaveLength(1); expect(last.active).not.toBe(tabs[0].key);
    });
    it('closing a background view preserves active selection and drafts', () => {
        const a = localTab(), b = localTab(); a.draft = 'draft';
        expect(closeTab({ tabs: [a, b], active: a.key }, b.key)).toEqual({ tabs: [a], active: a.key });
    });
    it('persists ordered per-server tabs without serializing browser files', () => {
        const storage = new Map<string, string>(); vi.stubGlobal('localStorage', { getItem: (k: string) => storage.get(k), setItem: (k: string, v: string) => storage.set(k, v) });
        const a = localTab(), b = localTab(); a.draft = 'one'; a.explicit.thinking = ''; a.files = [{} as File];
        saveTabs('one', { tabs: [a, b], active: b.key }); const restored = loadTabs('one');
        expect(restored.active).toBe(b.key); expect(restored.tabs.map(t => t.key)).toEqual([a.key, b.key]); expect(restored.tabs[0].draft).toBe('one'); expect(restored.tabs[0].explicit.thinking).toBe(''); expect(restored.tabs[0].files).toBeUndefined(); expect(loadTabs('two').tabs[0].key).not.toBe(a.key);
    });
    it('recovers from corrupt or unavailable storage', () => {
        vi.stubGlobal('localStorage', { getItem: () => '{', setItem: () => { throw new Error('private'); } });
        const state = loadTabs('server'); expect(state.tabs).toHaveLength(1); expect(() => saveTabs('server', state)).not.toThrow();
    });
});
