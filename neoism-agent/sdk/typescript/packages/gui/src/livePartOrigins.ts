import type { Event, MessageWithParts, Part, SessionRuntimeSnapshot } from '@neoism/sdk';

export type LivePartMap = ReadonlyMap<string, ReadonlySet<string>>;
export interface LivePartLedger {
    parts: LivePartMap;
    prompts: Set<string>;
    causal: Set<string>;
    createdHere: boolean;
}
export function livePartLedger(createdHere = false): LivePartLedger {
    return { parts: new Map(), prompts: new Set(), causal: new Set(), createdHere };
}
function mark(ledger: LivePartLedger, message: string, ids: string[]) {
    const prior = ledger.parts.get(message);
    const added = ids.filter(id => id && !prior?.has(id));
    if (!added.length) return;
    const next = new Map(ledger.parts); next.set(message, new Set([...(prior || []), ...added])); ledger.parts = next;
}
/** Called only after the reducer accepted a scoped, nonduplicate live event. */
export function markLiveEvent(ledger: LivePartLedger, event: Event) {
    if (event.type === 'message.part.updated') mark(ledger, event.data.part.messageId, [event.data.part.id]);
    if (event.type === 'message.part.delta') mark(ledger, event.data.messageID, [event.data.partID]);
}
/** Recover locally submitted work if HTTP history wins the SSE subscription race.
 * IDs are opaque: match explicit prompt/causal parent identities, never timestamps,
 * lexicographic order, author names, or "everything since send" guesses.
 */
export function markPromptSnapshot(ledger: LivePartLedger, messages: MessageWithParts[]) {
    if (ledger.createdHere) {
        messages.forEach(m => mark(ledger, m.info.id, m.parts.map(p => p.id)));
        return;
    }
    if (!ledger.prompts.size) return;
    const children = new Map<string, string[]>();
    messages.forEach(m => {
        const parent = m.info.parentId;
        if (typeof parent === 'string') children.set(parent, [...(children.get(parent) || []), m.info.id]);
    });
    const owned = new Set([...ledger.prompts, ...ledger.causal]), queue = [...owned];
    for (let i = 0; i < queue.length; i++) for (const child of children.get(queue[i]) || []) if (!owned.has(child)) { owned.add(child); queue.push(child); }
    ledger.causal = owned;
    messages.forEach(m => { if (owned.has(m.info.id)) mark(ledger, m.info.id, m.parts.map(p => p.id)); });
}
/** Snapshot recovery is restricted to the tail assistant, never an old unfinished row
 * behind a new prompt or a completed response. No timestamp/ID ordering heuristics. */
export function pendingSnapshotMessage(messages: MessageWithParts[], busy: boolean): string | undefined {
    if (!busy) return undefined;
    for (let i = messages.length - 1; i >= 0; i--) {
        const info = messages[i].info;
        if (info.role === 'user') return undefined;
        if (info.role !== 'assistant') continue;
        if (typeof info.time?.completed === 'number' || (info.finish && info.finish !== 'tool-calls')) return undefined;
        return info.id;
    }
    return undefined;
}
/** Runtime has child session IDs, not active tool-call IDs. Encode matching part IDs
 * as primitive row props so unchanged historical rows retain their memo boundary. */
export function outstandingTaskParts(messages: MessageWithParts[], runtime?: SessionRuntimeSnapshot, sessionId?: string): Map<string, string> {
    const found = new Map<string, string>();
    if (!runtime || runtime.rootSessionId !== sessionId) return found;
    const children = new Set(runtime.branches.filter(b => b.status === 'outstanding').map(b => b.sessionId));
    if (!children.size) return found;
    for (const message of messages) {
        const ids = message.parts.filter(p => {
            if (p.type !== 'tool' || p.state.status !== 'completed' || !/(?:^|[.:/])task$/.test(p.tool)) return false;
            for (const metadata of [p.state.metadata, p.metadata]) {
                if (!metadata || typeof metadata !== 'object') continue;
                const row = metadata as Record<string, unknown>;
                const child = row.childSessionId ?? row.sessionId ?? row.sessionID ?? row.session_id;
                if (typeof child === 'string' && children.has(child)) return true;
            }
            return false;
        }).map(p => p.id);
        if (ids.length) found.set(message.info.id, JSON.stringify(ids));
    }
    return found;
}
/** Primitive per-row runtime state, including terminal states for already SSE-visible cards.
 * An absent child in an authoritative snapshot is unknown, not historic 'running'. */
export function taskRuntimeStates(messages: MessageWithParts[], runtime?: SessionRuntimeSnapshot, sessionId?: string): Map<string, string> {
    const found = new Map<string, string>();
    if (!runtime || runtime.rootSessionId !== sessionId) return found;
    const children = new Map(runtime.branches.map(branch => [branch.sessionId, branch.status]));
    for (const message of messages) {
        const states: Array<[string, string]> = [];
        for (const part of message.parts) {
            if (part.type !== 'tool' || !/(?:^|[.:/])(?:task|task_result)$/.test(part.tool)) continue;
            for (const metadata of [part.state.metadata, part.metadata]) {
                if (!metadata || typeof metadata !== 'object') continue;
                const row = metadata as Record<string, unknown>;
                const child = row.childSessionId ?? row.sessionId ?? row.sessionID ?? row.session_id ?? row.sessID;
                if (typeof child !== 'string' || !child) continue;
                states.push([part.id, children.get(child) || 'unknown']); break;
            }
        }
        if (states.length) found.set(message.info.id, JSON.stringify(states));
    }
    return found;
}
export function visibleDetailParts(parts: Part[], live?: ReadonlySet<string>, recoverPending = false, outstanding?: ReadonlySet<string>) {
    return parts.filter(p => !['reasoning', 'tool', 'subtask'].includes(p.type) || !!live?.has(p.id) || !!outstanding?.has(p.id)
        || (recoverPending && (p.type === 'tool' && ['pending', 'running'].includes(p.state.status)
            || p.type === 'reasoning' && typeof p.time?.start === 'number' && typeof p.time?.end !== 'number')));
}
let lastPromptTime = -1, promptCounter = 0;
export function promptMessageId() {
    // Match neoism-agent-core::id::create(Message, Ascending): supported
    // request identity without changing the server's existing cursor format.
    const timestamp = Date.now();
    if (timestamp !== lastPromptTime) { lastPromptTime = timestamp; promptCounter = 0; }
    promptCounter = (promptCounter + 1) & 0xffff;
    const time = (BigInt(timestamp) * 0x1000n + BigInt(promptCounter)) & 0xffffffffffffn;
    const alphabet = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';
    const suffix = Array.from(crypto.getRandomValues(new Uint8Array(14)), byte => alphabet[byte % alphabet.length]).join('');
    return `msg_${time.toString(16).padStart(12, '0')}${suffix}`;
}
/** Session snapshots persist; visibility ledgers deliberately do not survive a view switch. */
export function liveOriginRegistry() {
    let current: { client: object; id?: string; ledger: LivePartLedger } | undefined;
    const created = new WeakMap<object, Map<string, LivePartLedger>>();
    return {
        view(client: object, id?: string) {
            if (current?.client === client && current.id === id) return current.ledger;
            const pending = id ? created.get(client)?.get(id) : undefined;
            if (id) created.get(client)?.delete(id);
            current = { client, id, ledger: pending || livePartLedger() };
            return current.ledger;
        },
        created(client: object, id: string) {
            if (current?.client === client && current.id === id) { current.ledger.createdHere = true; return; }
            let pending = created.get(client); if (!pending) { pending = new Map(); created.set(client, pending); }
            pending.set(id, livePartLedger(true));
        },
        prompt(client: object, id: string) {
            const messageId = promptMessageId();
            const ledger = current?.client === client && current.id === id ? current.ledger : created.get(client)?.get(id);
            ledger?.prompts.add(messageId);
            return messageId;
        },
    };
}
