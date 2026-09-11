import { describe, expect, it, vi } from "vitest";
import type { Event, NeoismClient } from "@neoism/sdk";
import { GUI_EVENT_QUEUE_LIMIT, GuiEventsOverflowError, subscribeGuiEvents } from "./sharedEvents";

const event = (sequence: number) => ({ sequence, type: "test", data: {} }) as unknown as Event;
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
function mock() {
    const streams: { signal: AbortSignal; emit(value: Event): void; end(): void; fail(error: unknown): void; returned: ReturnType<typeof vi.fn> }[] = [];
    const subscribe = vi.fn(({ signal }: { signal: AbortSignal }) => {
        let resolve: (value: IteratorResult<Event>) => void, reject: (error: unknown) => void;
        const returned = vi.fn(async () => { resolve?.({ done: true, value: undefined }); return { done: true as const, value: undefined }; });
        streams.push({ signal, emit: value => resolve({ done: false, value }), end: () => resolve({ done: true, value: undefined }), fail: error => reject(error), returned });
        return { [Symbol.asyncIterator]() { return this; }, next: () => new Promise<IteratorResult<Event>>((yes, no) => { resolve = yes; reject = no; }), return: returned };
    });
    return { client: { events: { subscribe } } as unknown as NeoismClient, streams, subscribe };
}

describe("GUI shared events", () => {
    it("is cold, shares reordered queries, and delivers identical references in order", async () => {
        const m = mock();
        const a = subscribeGuiEvents(m.client, { sessionId: "s", tail: true });
        const b = subscribeGuiEvents(m.client, { tail: true, sessionId: "s", since: undefined });
        a[Symbol.asyncIterator]();
        await tick(); expect(m.subscribe).not.toHaveBeenCalled();
        const firstA = a.next(), firstB = b.next();
        await tick(); expect(m.subscribe).toHaveBeenCalledTimes(1);
        const one = event(1); m.streams[0].emit(one);
        expect((await firstA).value).toBe(one); expect((await firstB).value).toBe(one);
        for (let i = 2; i <= 4; i++) { m.streams[0].emit(event(i)); await tick(); }
        for (let i = 2; i <= 4; i++) {
            const av = await a.next(), bv = await b.next();
            expect(av.value.sequence).toBe(i); expect(bv.value).toBe(av.value);
        }
        await a.return!(); expect(m.streams[0].signal.aborted).toBe(false);
        await b.return!(); await tick(); expect(m.streams[0].signal.aborted).toBe(true);
        expect(m.streams[0].returned).toHaveBeenCalledTimes(1);
    });

    it("independently aborts a pending consumer, and cancels only when all leave", async () => {
        const m = mock(), ac = new AbortController(), bc = new AbortController();
        const a = subscribeGuiEvents(m.client, { signal: ac.signal }), b = subscribeGuiEvents(m.client, { signal: bc.signal });
        const an = a.next(), bn = b.next(); await tick();
        ac.abort(); expect((await an).done).toBe(true); expect(m.streams[0].signal.aborted).toBe(false);
        m.streams[0].emit(event(1)); expect((await bn).value.sequence).toBe(1);
        const pending = b.next(); bc.abort(); expect((await pending).done).toBe(true);
        await tick(); expect(m.streams[0].signal.aborted).toBe(true); expect(m.streams[0].returned).toHaveBeenCalledTimes(1);
    });

    it("return unblocks a pending next without ending peers", async () => {
        const m = mock(), a = subscribeGuiEvents(m.client), b = subscribeGuiEvents(m.client);
        const an = a.next(), bn = b.next(); await tick();
        await a.return!(); expect((await an).done).toBe(true);
        m.streams[0].emit(event(1)); expect((await bn).value.sequence).toBe(1);
        await b.return!();
    });

    it("does not open for already aborted, returned, or same-turn aborted consumers", async () => {
        const m = mock(), ac = new AbortController(); ac.abort();
        expect((await subscribeGuiEvents(m.client, { signal: ac.signal }).next()).done).toBe(true);
        const returned = subscribeGuiEvents(m.client); await returned.return!(); expect((await returned.next()).done).toBe(true);
        const bc = new AbortController(), b = subscribeGuiEvents(m.client, { signal: bc.signal });
        const pending = b.next(); bc.abort(); expect((await pending).done).toBe(true);
        await tick(); expect(m.subscribe).not.toHaveBeenCalled();
    });

    it("isolates clients (servers/tokens), global/session scopes, and cursor/tail queries", async () => {
        const m = mock(), other = mock();
        const consumers = [subscribeGuiEvents(m.client), subscribeGuiEvents(m.client, { sessionId: "s" }),
            subscribeGuiEvents(m.client, { sessionId: "other" }), subscribeGuiEvents(m.client, { sessionId: "s", tail: true }),
            subscribeGuiEvents(m.client, { sessionId: "s", since: 1 }), subscribeGuiEvents(m.client, { sessionId: "s", since: 2 }),
            subscribeGuiEvents(other.client, { sessionId: "s" })];
        const pending = consumers.map(c => c.next()); await tick();
        expect(m.subscribe).toHaveBeenCalledTimes(6); expect(other.subscribe).toHaveBeenCalledTimes(1);
        await Promise.all(consumers.map(c => c.return!())); await Promise.all(pending);
    });

    it("late joiners receive only future events", async () => {
        const m = mock(), a = subscribeGuiEvents(m.client);
        const first = a.next(); await tick(); m.streams[0].emit(event(1)); await first;
        const b = subscribeGuiEvents(m.client), next = b.next(); await tick();
        m.streams[0].emit(event(2)); expect((await next).value.sequence).toBe(2);
        expect(m.subscribe).toHaveBeenCalledTimes(1); await a.return!(); await b.return!();
    });

    it("bounds slow queues with explicit resync error, preserving the fast consumer", async () => {
        const m = mock(), slow = subscribeGuiEvents(m.client), fast = subscribeGuiEvents(m.client);
        const initial = [slow.next(), fast.next()]; await tick(); m.streams[0].emit(event(0)); await Promise.all(initial);
        for (let i = 1; i <= GUI_EVENT_QUEUE_LIMIT + 1; i++) {
            const next = fast.next(); m.streams[0].emit(event(i)); expect((await next).value.sequence).toBe(i);
        }
        await expect(slow.next()).rejects.toBeInstanceOf(GuiEventsOverflowError);
        expect(m.streams[0].signal.aborted).toBe(false);
        await fast.return!(); await tick(); expect(m.streams[0].signal.aborted).toBe(true);
    });

    it("shares an immediately yielding source during the initial consumption window", async () => {
        const values = [event(1), event(2)];
        const subscribe = vi.fn(async function* () { yield* values; });
        const client = { events: { subscribe } } as unknown as NeoismClient;
        const a = subscribeGuiEvents(client), b = subscribeGuiEvents(client);
        const collect = async (source: AsyncIterable<Event>) => { const result = []; for await (const value of source) result.push(value); return result; };
        expect(await Promise.all([collect(a), collect(b)])).toEqual([values, values]);
        expect(subscribe).toHaveBeenCalledTimes(1);
    });

    it("overflow of the last consumer releases the source", async () => {
        const m = mock(), slow = subscribeGuiEvents(m.client);
        const first = slow.next(); await tick(); m.streams[0].emit(event(0)); await first;
        for (let i = 0; i <= GUI_EVENT_QUEUE_LIMIT; i++) { m.streams[0].emit(event(i)); await tick(); }
        await expect(slow.next()).rejects.toThrow("resync");
        expect(m.streams[0].signal.aborted).toBe(true); expect(m.streams[0].returned).toHaveBeenCalledTimes(1);
    });

    it("old cancelled generation cannot retire its replacement", async () => {
        const m = mock(), old = subscribeGuiEvents(m.client);
        const oldNext = old.next(); await tick();
        void old.return!();
        const fresh = subscribeGuiEvents(m.client), freshNext = fresh.next();
        await tick(); await oldNext;
        const peer = subscribeGuiEvents(m.client), peerNext = peer.next(); await tick();
        expect(m.subscribe).toHaveBeenCalledTimes(2);
        m.streams[1].emit(event(1)); await Promise.all([freshNext, peerNext]);
        await fresh.return!(); await peer.return!();
    });

    it.each([false, true])("retires completed/failed sources and opens a fresh generation (error=%s)", async failed => {
        const m = mock(), a = subscribeGuiEvents(m.client), b = subscribeGuiEvents(m.client);
        const first = a.next(), second = b.next(); await tick();
        const error = new Error("source failed");
        if (failed) {
            m.streams[0].fail(error);
            await expect(first).rejects.toBe(error); await expect(second).rejects.toBe(error);
        } else { m.streams[0].end(); expect((await first).done).toBe(true); expect((await second).done).toBe(true); }
        await tick(); expect(m.streams[0].returned).toHaveBeenCalledTimes(1);
        const fresh = subscribeGuiEvents(m.client), pending = fresh.next(); await tick();
        expect(m.subscribe).toHaveBeenCalledTimes(2); m.streams[1].emit(event(9)); expect((await pending).value.sequence).toBe(9);
        await fresh.return!();
    });
});
