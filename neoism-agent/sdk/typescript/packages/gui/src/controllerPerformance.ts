import { useCallback, useRef } from 'react';
import type { MessageWithParts, ProviderListResult } from '@neoism/sdk';
import type { Choice } from './components/Composer';

/** Stable UI callback identity without capturing a stale tab/server selection. */
export function useEventCallback<Args extends unknown[], Result>(handler: (...args: Args) => Result) {
    const latest = useRef(handler); latest.current = handler;
    return useCallback((...args: Args) => latest.current(...args), []);
}

/** Identity cache for immutable SDK snapshots; weak keys never retain old catalogs. */
const catalogs = new WeakMap<ProviderListResult, Choice[]>();
export function modelChoices(catalog: ProviderListResult): Choice[] {
    const cached = catalogs.get(catalog);
    if (cached) return cached;
    const connected = new Set(catalog.connected);
    const rank = (id: string) => { const index = ['opencode', 'openai', 'anthropic', 'claude-code'].indexOf(id); return index < 0 ? 4 : index; };
    const providers = [...catalog.all].sort((a, b) => rank(a.id) - rank(b.id) || a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
    const free = (p: string, m: ProviderListResult['all'][number]['models'][string]) => {
        const cost = m.cost as { input?: unknown; output?: unknown } | undefined;
        return p === 'opencode' && cost?.input === 0 && cost?.output === 0;
    };
    const choices = providers.flatMap(p => Object.values(p.models).sort((a, b) => Number(free(p.id, b)) - Number(free(p.id, a)) || a.name.toLowerCase().localeCompare(b.name.toLowerCase())).map(m => ({
        id: p.id + '/' + m.id, label: m.name, badge: free(p.id, m) ? 'Free' : undefined,
        description: p.name + (connected.has(p.id) ? '' : ' · not connected'), section: p.name,
    })));
    catalogs.set(catalog, choices);
    return choices;
}

/** One pass, no temporary flattened array of every text/tool part. */
export function messageUsage(messages: MessageWithParts[]) {
    return messages.flatMap(m => m.parts.filter(p => p.type === 'step-finish'));
}

/** Coalesce mutations outside input handlers; flush on pagehide/unmount. No history or files. */
export function deferredPersistence<T>(write: (key: string, value: T) => void) {
    const pending = new Map<string, T>();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const flush = () => {
        if (timer !== undefined) clearTimeout(timer);
        timer = undefined;
        const entries = [...pending];
        pending.clear();
        entries.forEach(([key, value]) => write(key, value));
    };
    return {
        schedule(key: string, value: T) {
            pending.set(key, value);
            if (timer === undefined) timer = setTimeout(flush, 0);
        },
        flush,
    };
}
