import type { Event, NeoismClient } from "@neoism/sdk";

type Options = NonNullable<Parameters<NeoismClient["events"]["subscribe"]>[0]>;
type Result = IteratorResult<Event>;
type Listener = { push(event: Event): void; finish(error?: unknown, failed?: boolean): void };
type Group = { listeners: Set<Listener>; stop(): void };
const groups = new WeakMap<NeoismClient, Map<string, Group>>();
export const GUI_EVENT_QUEUE_LIMIT = 256;
export class GuiEventsOverflowError extends Error {
    constructor() { super("Live event consumer fell behind; refetch initial state and resubscribe to resync."); this.name = "GuiEventsOverflowError"; }
}

/** GUI-only, cold until next(). Client identity and all defined query options (not signal)
 * form the key; different since/after cursors never share. No cache/replay: late joiners
 * receive future events only and must fetch initial state. Event references are read-only.
 * Reconnection belongs exclusively to the SDK; a finished source retires its group.
 */
export function subscribeGuiEvents(client: NeoismClient, options: Options = {}): AsyncIterableIterator<Event> {
    const { signal, ...query } = options;
    const key = JSON.stringify(Object.entries(query).filter(([, value]) => value !== undefined).sort(([a], [b]) => a.localeCompare(b)));
    const queue: Event[] = [];
    const waiting: { resolve(value: Result): void; reject(error: unknown): void }[] = [];
    let group: Group | undefined, attached = false, ended = false, failed = false, failure: unknown;
    const done: Result = { done: true, value: undefined };
    function detach() {
        signal?.removeEventListener("abort", abort);
        if (group) {
            group.listeners.delete(listener);
            if (!group.listeners.size) group.stop();
            group = undefined;
        }
    }
    function finish(error?: unknown, isError = false) {
        if (ended) return;
        ended = true; failed = isError; failure = error;
        detach();
        for (const waiter of waiting.splice(0)) isError ? waiter.reject(error) : waiter.resolve(done);
    }
    function abort() { queue.length = 0; finish(); }
    const listener: Listener = {
        finish,
        push(event) {
            if (ended) return;
            const waiter = waiting.shift();
            if (waiter) waiter.resolve({ done: false, value: event });
            else if (queue.length < GUI_EVENT_QUEUE_LIMIT) queue.push(event);
            else { queue.length = 0; finish(new GuiEventsOverflowError(), true); }
        },
    };
    function attach() {
        attached = true;
        if (signal?.aborted) { abort(); return; }
        signal?.addEventListener("abort", abort, { once: true });
        let pool = groups.get(client);
        if (!pool) { pool = new Map(); groups.set(client, pool); }
        group = pool.get(key);
        if (!group) {
            const controller = new AbortController();
            let source: AsyncIterator<Event> | undefined, stopped = false;
            const created: Group = {
                listeners: new Set(),
                stop() {
                    if (stopped) return;
                    stopped = true;
                    if (pool.get(key) === created) pool.delete(key);
                    controller.abort();
                    // Abort unblocks the SDK's pending read; return also releases custom sources.
                    if (source?.return) void Promise.resolve().then(() => source!.return!()).catch(() => {});
                },
            };
            group = created;
            pool.set(key, created);
            // Let synchronous consumers attach before even an immediately yielding source opens.
            queueMicrotask(() => { void (async () => {
                try {
                    if (stopped) return;
                    source = client.events.subscribe({ ...query, signal: controller.signal })[Symbol.asyncIterator]();
                    while (!stopped) {
                        const result = await source.next();
                        if (result.done || stopped) break;
                        for (const consumer of created.listeners) consumer.push(result.value);
                    }
                } catch (error) {
                    for (const consumer of [...created.listeners]) consumer.finish(error, true);
                } finally {
                    created.stop();
                    for (const consumer of [...created.listeners]) consumer.finish();
                }
            })(); });
        }
        group.listeners.add(listener);
    }
    return {
        [Symbol.asyncIterator]() { return this; },
        next() {
            if (!attached && !ended) attach();
            if (queue.length) return Promise.resolve({ done: false as const, value: queue.shift()! });
            if (ended) return failed ? Promise.reject(failure) : Promise.resolve(done);
            return new Promise<Result>((resolve, reject) => waiting.push({ resolve, reject }));
        },
        return() { abort(); return Promise.resolve(done); },
    };
}
