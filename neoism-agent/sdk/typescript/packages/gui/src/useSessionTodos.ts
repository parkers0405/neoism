import { subscribeGuiEvents } from "./sharedEvents";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Event, MessageWithParts, NeoismClient } from "@neoism/sdk";
import { latestTodoSnapshot, parseTodos, reconcileTodos, type SessionTodo } from "./todoHelpers";

const EMPTY: readonly SessionTodo[] = [];
const EMPTY_MESSAGES: readonly MessageWithParts[] = [];
export interface SessionTodosOptions {
    /** Immutable useChat state; used only until hydration or on older servers. */
    messages?: readonly MessageWithParts[];
    /** Include server/account identity if the transport can change without a new client. */
    serverKey?: string;
    /** False when the parent forwards its existing SSE stream to onEvent. */
    subscribe?: boolean;
}
export function useSessionTodos(client: NeoismClient, sessionId: string | undefined, options: SessionTodosOptions = {}) {
    const { messages = EMPTY_MESSAGES, serverKey = "", subscribe = true } = options;
    const scope = useMemo(() => ({ client, sessionId, serverKey, active: true }), [client, sessionId, serverKey]);
    const current = useRef(scope); current.current = scope;
    const [state, setState] = useState<{ scope: typeof scope; todos?: readonly SessionTodo[]; loading: boolean; error?: string }>();
    const live = useRef<{ scope: typeof scope; revision: number; sequence: number; source?: string }>({ scope, revision: 0, sequence: -1 });
    if (live.current.scope !== scope) live.current = { scope, revision: 0, sequence: -1 };
    const onEvent = useCallback((event: Event) => {
        if (!scope.active || current.current !== scope || event.type !== "todo.updated" || event.data.sessionID !== sessionId) return;
        const todos = parseTodos(event.data.todos);
        if (!todos || live.current.source === event.source && event.sequence <= live.current.sequence) return;
        live.current.revision++; live.current.sequence = event.sequence; live.current.source = event.source;
        setState(old => ({ scope, todos: reconcileTodos(old?.scope === scope ? old.todos ?? EMPTY : EMPTY, todos), loading: false }));
    }, [scope, sessionId]);
    useEffect(() => {
        scope.active = true;
        if (!sessionId) return;
        const abort = new AbortController();
        const valid = () => !abort.signal.aborted && current.current === scope;
        const revision = live.current.revision;
        setState(old => old?.scope === scope ? old : { scope, loading: true });
        if (subscribe) void (async () => {
            try {
                for await (const event of subscribeGuiEvents(client, { sessionId, tail: true, signal: abort.signal })) {
                    if (!valid()) break;
                    onEvent(event);
                }
            } catch (error) {
                if (valid()) setState(old => ({ ...old, scope, loading: old?.loading ?? false, error: String(error) }));
            }
        })();
        void client.operations.request("v2.sessions.todos", { path: { session_id: sessionId }, signal: abort.signal }).then(payload => {
            if (!valid() || live.current.revision !== revision) return;
            const todos = parseTodos(payload);
            setState(old => ({ scope, todos: todos === undefined ? undefined : reconcileTodos(old?.scope === scope ? old.todos ?? EMPTY : EMPTY, todos), loading: false,
                ...(todos === undefined ? { error: "Invalid task response" } : {}) }));
        }).catch(error => {
            if (!valid() || live.current.revision !== revision) return;
            const status = error && typeof error === "object" ? error.status : undefined;
            setState({ scope, loading: false, error: [404, 405, 501].includes(status) ? undefined : String(error) });
        });
        return () => { scope.active = false; abort.abort(); };
    }, [client, sessionId, scope, subscribe, onEvent]);
    const fallback = useMemo(() => sessionId ? latestTodoSnapshot(messages, sessionId) : undefined, [messages, sessionId]);
    const stable = useRef<{ scope: typeof scope; todos: readonly SessionTodo[] }>({ scope, todos: EMPTY });
    const data = state?.scope === scope ? state : undefined;
    const next = data?.todos ?? fallback?.todos ?? EMPTY;
    stable.current = { scope, todos: reconcileTodos(stable.current.scope === scope ? stable.current.todos : EMPTY, next) };
    return { todos: stable.current.todos, loading: !!sessionId && (data?.loading ?? true), error: data?.error,
        source: data?.todos !== undefined ? "server" as const : "messages" as const,
        latestPartId: fallback?.partId, onEvent };
}
