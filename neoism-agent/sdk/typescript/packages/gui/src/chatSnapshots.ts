import { useCallback, useRef, useState, type SetStateAction } from 'react';
import type { NeoismClient } from '@neoism/sdk';
import { emptyChat, type ChatState } from './state';

export interface ChatSnapshot { state: ChatState; older?: string; loading: boolean; fetchedAt?: number }
export const CHAT_FRESH_MS = 15_000;
/** Per-hook, per-transport cache. Credential changes create a different client and
 * cannot expose another credential's transcript, even on the render before effects.
 */
export function useChatSnapshot(client: NeoismClient, id?: string) {
    const stores = useRef(new WeakMap<NeoismClient, Map<string | undefined, ChatSnapshot>>());
    let store = stores.current.get(client);
    if (!store) { store = new Map(); stores.current.set(client, store); }
    let snapshot = store.get(id);
    if (!snapshot) {
        snapshot = { state: emptyChat, loading: !!id };
        store.set(id, snapshot);
        // Bound inactive transcript retention (including loaded older pages).
        if (store.size > 12) store.delete(store.keys().next().value);
    } else { store.delete(id); store.set(id, snapshot); }
    const [, changed] = useState(0);
    const setState = useCallback((next: SetStateAction<ChatState>, session: string | undefined = id) => {
        // An awaited create/prompt may still hold the unsent view's setter.
        // Explicit targets update that session, never the cached empty draft.
        let target = session === id ? snapshot : store.get(session);
        if (!target) {
            target = { state: emptyChat, loading: !!session }; store.set(session, target);
            if (store.size > 12) store.delete(store.keys().next().value);
        }
        const value = typeof next === 'function' ? next(target.state) : next;
        if (value !== target.state) { target.state = value; changed(n => n + 1); }
    }, [snapshot, store, id]);
    const setOlder = useCallback((value: string | undefined) => {
        if (snapshot.older !== value) { snapshot.older = value; changed(n => n + 1); }
    }, [snapshot]);
    const setLoading = useCallback((value: boolean) => {
        if (snapshot.loading !== value) { snapshot.loading = value; changed(n => n + 1); }
    }, [snapshot]);
    return { snapshot, state: snapshot.state, older: snapshot.older, loading: snapshot.loading, setState, setOlder, setLoading };
}
