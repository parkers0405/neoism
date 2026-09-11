import { subscribeGuiEvents } from "./sharedEvents";
import { useSessionActivity } from "./useSessionActivity";
import { useCallback, useEffect, useMemo, useRef } from "react";
import { liveOriginRegistry, markLiveEvent, markPromptSnapshot } from "./livePartOrigins";
import { CHAT_FRESH_MS, useChatSnapshot } from "./chatSnapshots";
import type { NeoismClient, Event } from "@neoism/sdk";
import {
    applyRuntime,
    runtimeWorking,
    historyCursor,
    reconcile,
    reconcileRecent,
    reduceEvent,
} from "./state";
import { errorMessage } from "./types";
export function useChat(
    client: NeoismClient,
    id: string | undefined,
    notify: (error: string) => void,
) {
    const { snapshot, state, setState, older, setOlder, loading, setLoading } = useChatSnapshot(client, id);
    const sessionActivity = useSessionActivity(client, id, state.runtime);
    const activityEvent = useRef(sessionActivity.onEvent); activityEvent.current = sessionActivity.onEvent;
    const origins = useMemo(() => liveOriginRegistry(), []);
    const ledger = origins.view(client, id);
    const markCreatedSession = useCallback((session: string) => origins.created(client, session), [client, origins]);
    const beginPrompt = useCallback((session: string) => origins.prompt(client, session), [client, origins]);
    const epoch = useRef(0);
    const olderRequest = useRef<number | undefined>(undefined);
    useEffect(() => {
        const generation = ++epoch.current;
        setLoading(!!id && snapshot.fetchedAt === undefined);
        if (!id) return;
        const controller = new AbortController();
        let fetching = false;
        let concurrentEvents = false;
        let statusVersion = 0;
        let pendingRefresh = false;
        let initialized = snapshot.fetchedAt !== undefined;
        const refresh = async () => {
            if (fetching) {
                pendingRefresh = true;
                return;
            }
            fetching = true;
            concurrentEvents = false;
            try {
                const page = await client.operations.request(
                    "v2.sessions.messages",
                    {
                        path: { session_id: id },
                        query: { limit: 50, order: "desc" },
                        signal: controller.signal,
                    },
                );
                if (controller.signal.aborted) return;
                // A history request and SSE have no shared snapshot sequence. If events raced
                // the fetch, keep the live state rather than replay deltas onto an already
                // updated snapshot (which would duplicate text). The next quiet poll reconciles.
                const raced = concurrentEvents;
                markPromptSnapshot(ledger, page.items);
                setState((s) => reconcileRecent(s, page.items, raced));
                if (!initialized) {
                    setOlder(historyCursor(page.items, 50, undefined, page.cursor.next));
                    setLoading(false);
                    initialized = true;
                }
                snapshot.fetchedAt = Date.now();
                const statusAtRequest = statusVersion;
                const [statuses, runtime] = await Promise.all([
                    client.sessions.status(),
                    client.operations.request("v2.sessions.runtime", {
                        path: { session_id: id }, signal: controller.signal,
                    }),
                ]);
                if (!controller.signal.aborted) setState((s) => applyRuntime(s, runtime));
                if (!controller.signal.aborted && statusVersion === statusAtRequest)
                    setState((s) => ({
                        ...s,
                        busy: statuses[id]?.type !== "idle" && !!statuses[id],
                    }));
            } catch (e) {
                if (!controller.signal.aborted) {
                    notify(errorMessage(e));
                    if (!initialized) setLoading(false);
                }
            } finally {
                fetching = false;
                if (pendingRefresh && !controller.signal.aborted) {
                    pendingRefresh = false;
                    void refresh();
                }
            }
        };
        void (async () => {
            try {
                for await (const event of subscribeGuiEvents(client, {
                    sessionId: id,
                    tail: true,
                    signal: controller.signal,
                })) {
                    if (controller.signal.aborted) break;
                    activityEvent.current(event);
                    if (fetching && event.type.startsWith("message.")) concurrentEvents = true;
                    if (event.type === "session.status") statusVersion++;
                    setState((s) => {
                        const next = reduceEvent(s, event, id);
                        if (next !== s) markLiveEvent(ledger, event);
                        return next;
                    });
                    if (
                        event.type === "session.status" && event.data.status.type === "idle" ||
                        event.type === "session.subtask.completed" ||
                        event.type === "session.background_tasks.updated" ||
                        event.type === "session.background_task.completed"
                    ) void refresh();
                }
            } catch (e) {
                if (!controller.signal.aborted) notify(errorMessage(e));
            }
        })();
        if (snapshot.fetchedAt === undefined || Date.now() - snapshot.fetchedAt >= CHAT_FRESH_MS || snapshot.state.busy || runtimeWorking(snapshot.state.runtime)) void refresh();
        // Reconcile after reconnects (SDK reconnects internally), plus missed idle/status events.
        // A fixed, single-flight interval never cascades into pagination requests.
        const timer = setInterval(() => {
            if (!document.hidden) void refresh();
        }, 15000);
        const focus = () => void refresh();
        window.addEventListener("focus", focus);
        return () => {
            controller.abort();
            clearInterval(timer);
            window.removeEventListener("focus", focus);
            if (epoch.current === generation) epoch.current++;
        };
    }, [client, id, notify]);
    const loadOlder = useCallback(async () => {
        if (!id || !older || loading) return;
        const generation = epoch.current;
        if (olderRequest.current === generation) return;
        olderRequest.current = generation;
        setLoading(true);
        try {
            const page = await client.operations.request(
                "v2.sessions.messages",
                {
                    path: { session_id: id },
                    query: { cursor: older, order: "desc", limit: 50 },
                },
            );
            if (epoch.current !== generation) return;
            setState((s) => reconcile(s, page.items));
            setOlder(historyCursor(page.items, 50, older, page.cursor.next));
        } catch (e) {
            if (epoch.current === generation) notify(errorMessage(e));
        } finally {
            if (olderRequest.current === generation) olderRequest.current = undefined;
            if (epoch.current === generation) setLoading(false);
        }
    }, [client, id, older, loading, notify]);
    return {
        sessionActivity,
        activityBusy: state.busy, // Own session status, before runtime children/jobs are folded into UI busy.
        state: { ...state, busy: state.busy || runtimeWorking(state.runtime) },
        setState, older, loading, loadOlder,
        liveParts: ledger.parts, beginPrompt, markCreatedSession,
    };
}
