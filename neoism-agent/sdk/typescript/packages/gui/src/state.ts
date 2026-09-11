import type { Event, MessageWithParts, Part, SessionRuntimeSnapshot } from "@neoism/sdk";
export interface ChatState {
    messages: MessageWithParts[];
    busy: boolean;
    runtime?: SessionRuntimeSnapshot;
    removedMessages?: string[];
    /** Membership of the last quiet recent-history snapshot (not display order). */
    recentMessageIds?: string[];
    error?: string;
    seen: string[];
}
export const emptyChat: ChatState = { messages: [], busy: false, seen: [] };
export function runtimeWorking(runtime?: SessionRuntimeSnapshot): boolean {
    return !!runtime && (
        !!runtime.execution && !runtime.execution.finished ||
        runtime.branches.some((branch) => branch.status === "outstanding") ||
        !!runtime.runningBackgroundTasks?.length
    );
}
export function applyRuntime(state: ChatState, runtime: SessionRuntimeSnapshot): ChatState {
    if (state.runtime && state.runtime.revision > runtime.revision) return state;
    return { ...state, runtime };
}
export function mergePage<T>(
    old: T[],
    incoming: T[],
    key: (item: T) => string,
): T[] {
    const map = new Map(old.map((x) => [key(x), x]));
    incoming.forEach((x) => map.set(key(x), x));
    return [...map.values()];
}
export function nextCursor(
    current: string | undefined,
    next: string | undefined,
): string | undefined {
    return next && next !== current ? next : undefined;
}
export function historyCursor(
    messages: MessageWithParts[],
    pageSize: number,
    current?: string,
    next?: string,
): string | undefined {
    // Older V2 servers accept a message ID cursor but return an empty page cursor.
    return nextCursor(current, next ?? (messages.length === pageSize ? messages.at(-1)?.info.id : undefined));
}
/** Server milliseconds only. Missing/invalid metadata is not a local-clock event. */
export function messageCreated(message: MessageWithParts): number | undefined {
    const created = message.info.time?.created;
    return typeof created === "number" && Number.isFinite(created) && created >= 0
        ? created : undefined;
}

/**
 * Stable server-time chronology with only parent-before-child constraints.
 * A parent does NOT own a contiguous display turn: steering users must remain
 * between its earlier and later assistant steps. Ties retain input order.
 * Unknown times trail known times; IDs and local clocks never supply an age.
 * Among currently eligible rows, emit the earliest chronological/input rank.
 */
export function orderMessages(messages: MessageWithParts[]): MessageWithParts[] {
    const ordered = [...messages].sort((a, b) =>
        (messageCreated(a) ?? Infinity) - (messageCreated(b) ?? Infinity) || 0);
    const ranks = new Map(ordered.map((message, index) => [message.info.id, index]));
    const children = new Map<number, number[]>();
    const blocked = new Set<number>();
    for (const [index, message] of ordered.entries()) {
        const parentId = message.info.parentId;
        const parent = typeof parentId === "string" ? ranks.get(parentId) : undefined;
        if (parent === undefined || parent === index) continue;
        const dependents = children.get(parent) ?? [];
        dependents.push(index);
        children.set(parent, dependents);
        blocked.add(index);
    }
    // Min-heap of chronological ranks keeps the topological pass O(n log n),
    // including large transcripts with many steps parented to one user.
    const ready: number[] = [];
    const push = (rank: number) => {
        let index = ready.length;
        ready.push(rank);
        while (index > 0) {
            const parent = (index - 1) >> 1;
            if (ready[parent] <= rank) break;
            ready[index] = ready[parent];
            index = parent;
        }
        ready[index] = rank;
    };
    const pop = () => {
        const rank = ready[0];
        const tail = ready.pop()!;
        if (ready.length) {
            let index = 0;
            while (index * 2 + 1 < ready.length) {
                let child = index * 2 + 1;
                if (child + 1 < ready.length && ready[child + 1] < ready[child]) child++;
                if (ready[child] >= tail) break;
                ready[index] = ready[child];
                index = child;
            }
            ready[index] = tail;
        }
        return rank;
    };
    ordered.forEach((_, index) => { if (!blocked.has(index)) push(index); });
    const emitted = new Set<number>();
    const result: MessageWithParts[] = [];
    let fallback = 0;
    while (result.length < ordered.length) {
        if (!ready.length) {
            // Malformed cycles cannot satisfy every edge. Break one at the
            // earliest remaining rank, preserving all rows without hanging.
            while (emitted.has(fallback)) fallback++;
            push(fallback);
        }
        const rank = pop();
        if (emitted.has(rank)) continue;
        emitted.add(rank);
        result.push(ordered[rank]);
        for (const child of children.get(rank) ?? []) {
            if (!emitted.has(child)) push(child);
        }
    }
    return result;
}

export function reconcile(
    state: ChatState,
    messages: MessageWithParts[],
): ChatState {
    const removed = new Set(state.removedMessages);
    return {
        ...state,
        messages: orderMessages(mergePage(state.messages, messages, (m) => m.info.id)
            .filter((m) => !removed.has(m.info.id))),
    };
}
export function reconcileRecent(
    state: ChatState,
    messages: MessageWithParts[],
    concurrentEvents = false,
): ChatState {
    // No SSE watermark: never overwrite live bytes with a racing snapshot.
    // Only admit provably older, previously unseen rows; IDs prove no age.
    const removed = new Set(state.removedMessages);
    const snapshot = messages.filter((m) => !removed.has(m.info.id));
    if (concurrentEvents) {
        const times = state.messages.map(messageCreated).filter((t): t is number => t !== undefined);
        const firstLive = times.length ? times.reduce((a, b) => Math.min(a, b)) : undefined;
        const liveIds = new Set(state.messages.map((m) => m.info.id));
        const older = firstLive === undefined ? [] : snapshot.filter((m) => {
            const created = messageCreated(m);
            return !liveIds.has(m.info.id) && created !== undefined && created < firstLive;
        });
        return older.length ? reconcile(state, older) : state;
    }
    // A limited page is not an authoritative list of all loaded history. Only
    // infer deletion for rows previously in a quiet snapshot and still inside
    // this snapshot's strict time window. Preserve unknown/equal-time rows and
    // paged-out older rows. Explicit SSE tombstones always take precedence.
    const ids = new Set(snapshot.map((m) => m.info.id));
    const priorIds = new Set(state.recentMessageIds);
    const times = snapshot.map(messageCreated);
    const floor = times.length && times.every((t) => t !== undefined)
        ? (times as number[]).reduce((a, b) => Math.min(a, b)) : undefined;
    const retained = messages.length === 0 ? [] : state.messages.filter((m) => {
        const created = messageCreated(m);
        return ids.has(m.info.id) || !priorIds.has(m.info.id) || floor === undefined
            || created === undefined || created <= floor;
    });
    return {
        ...reconcile({ ...state, messages: retained }, snapshot),
        recentMessageIds: [...ids],
    };
}
export function reduceEvent(
    state: ChatState,
    event: Event,
    sessionId: string,
): ChatState {
    if (state.seen.includes(event.id)) return state;
    const data = event.data as Record<string, unknown>;
    if (
        (data.sessionID ??
            data.sessionId ??
            (event.subject?.kind === "session"
                ? event.subject.id
                : undefined)) !== sessionId
    )
        return state;
    let next = { ...state, seen: [...state.seen.slice(-2047), event.id] };
    switch (event.type) {
        case "session.execution.updated":
            return applyRuntime(next, event.data.runtime);
        case "session.status":
            return { ...next, busy: event.data.status.type !== "idle" };
        case "session.error":
            return { ...next, busy: false, error: event.data.error.message };
        case "message.removed":
            return {
                ...next,
                removedMessages: [...(next.removedMessages || []).slice(-2047), event.data.messageID],
                messages: next.messages.filter(
                    (m) => m.info.id !== event.data.messageID,
                ),
            };
        case "message.updated": {
            const info = event.data.info;
            if (info.role !== "user" && info.role !== "assistant") return next;
            if (next.removedMessages?.includes(info.id)) return next;
            const prior = next.messages.find((m) => m.info.id === info.id);
            return reconcile(next, [
                {
                    info: {
                        ...prior?.info,
                        ...info,
                        role: info.role,
                        time: { ...prior?.info.time, ...(info.time as Record<string, unknown>) },
                    },
                    parts: prior?.parts || [],
                },
            ]);
        }
        case "message.part.updated": {
            const part = event.data.part;
            const prior = next.messages.find(
                (m) => m.info.id === part.messageId,
            );
            const msg: MessageWithParts = prior || {
                info: {
                    id: part.messageId,
                    sessionId,
                    role: part.role === "user" ? "user" : "assistant",
                    time: {},
                },
                parts: [],
            };
            return reconcile(next, [
                { ...msg, parts: mergePage(msg.parts, [part], (p) => p.id) },
            ]);
        }
        case "message.part.removed":
            return {
                ...next,
                messages: next.messages.map((m) => ({
                    ...m,
                    parts: m.parts.filter((p) => p.id !== event.data.partID),
                })),
            };
        case "message.part.delta": {
            const { messageID, partID, delta, field, partType } = event.data;
            const prior = next.messages.find((m) => m.info.id === messageID);
            const msg: MessageWithParts = prior || {
                info: { id: messageID, sessionId, role: "assistant", time: {} },
                parts: [],
            };
            const part = msg.parts.find((p) => p.id === partID);
            if (!part && !["text", "reasoning"].includes(partType)) return next;
            const updated = {
                ...(part || {
                    id: partID,
                    messageId: messageID,
                    sessionId,
                    type: partType,
                }),
                [field]: String(part?.[field] || "") + delta,
            } as Part;
            return reconcile(next, [
                { ...msg, parts: mergePage(msg.parts, [updated], (p) => p.id) },
            ]);
        }
        default:
            return next;
    }
}
