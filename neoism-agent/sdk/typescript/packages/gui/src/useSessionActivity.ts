import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Event, NeoismClient, SessionQueueInfo, SessionRuntimeSnapshot } from "@neoism/sdk";

const EMPTY_JOBS: NonNullable<SessionRuntimeSnapshot["runningBackgroundTasks"]> = [];
const EMPTY_STOPPING: string[] = [];

/** Uses useChat's existing event stream; token deltas never trigger a request. */
export function useSessionActivity(client: NeoismClient, id: string | undefined, runtime?: SessionRuntimeSnapshot) {
    const scope = useMemo(() => ({ client, id, controller: new AbortController(), version: 0, pending: false }), [client, id]);
    const current = useRef(scope); current.current = scope;
    const [snapshot, setSnapshot] = useState<{ scope: typeof scope; queue?: SessionQueueInfo; error?: string; pending?: string; stopping?: string[] }>();
    const valid = useCallback(() => current.current === scope && !scope.controller.signal.aborted, [scope]);
    const patch = useCallback((value: Partial<NonNullable<typeof snapshot>>) => {
        if (valid()) setSnapshot(old => ({ ...(old?.scope === scope ? old : {}), scope, ...value }));
    }, [scope, valid]);
    const refresh = useCallback(async () => {
        if (!id || !valid() || scope.pending) return;
        scope.pending = true;
        const version = scope.version, controller = scope.controller;
        try {
            const queue = await client.operations.request("v2.sessions.queue.list", { path: { session_id: id }, signal: controller.signal });
            if (scope.controller === controller && scope.version === version) patch({ queue, error: undefined });
        } catch (e) { if (scope.controller === controller) patch({ error: e instanceof Error ? e.message : String(e) }); }
        finally { if (scope.controller === controller) scope.pending = false; }
    }, [client, id, scope, valid, patch]);
    useEffect(() => {
        // React StrictMode replays setup/cleanup without recreating memoized scope.
        if (scope.controller.signal.aborted) { scope.controller = new AbortController(); scope.version++; scope.pending = false; }
        void refresh();
        const timer = setInterval(() => { if (!document.hidden) void refresh(); }, 15000);
        const focus = () => void refresh(); window.addEventListener("focus", focus);
        return () => { scope.controller.abort(); clearInterval(timer); window.removeEventListener("focus", focus); };
    }, [scope, refresh]);
    const onEvent = useCallback((event: Event) => {
        if (event.type === "session.queue.updated" && event.data.sessionID === id) {
            scope.version++; patch({ queue: event.data.queue, error: undefined });
        } else if (event.type === "session.status" && event.data.sessionID === id) void refresh();
    }, [id, scope, patch, refresh]);
    const data = snapshot?.scope === scope ? snapshot : undefined;
    const stopping = data?.stopping ?? EMPTY_STOPPING;
    const jobs = runtime?.runningBackgroundTasks ?? EMPTY_JOBS;
    const lock = useRef(false);
    useEffect(() => { lock.current = false; }, [scope]);
    const mutate = useCallback(async (action: "pop" | "clear" | "stop", job?: { sessionId: string; jobId: string }) => {
        if (!id || !valid() || lock.current) return;
        lock.current = true;
        const key = job ? `${job.sessionId}/${job.jobId}` : action;
        patch({ pending: key, error: undefined });
        const version = ++scope.version;
        let succeeded = false;
        try {
            if (action === "stop" && job) {
                await client.operations.request("v2.sessions.jobs.cancel", { path: { session_id: job.sessionId, job_id: job.jobId }, signal: scope.controller.signal });
                patch({ stopping: [...stopping, key] });
            } else if (action !== "stop") {
                const result = await client.operations.request(action === "pop" ? "v2.sessions.queue.pop" : "v2.sessions.queue.clear", { path: { session_id: id }, signal: scope.controller.signal });
                if (version === scope.version) patch({ queue: result.queue });
            }
            succeeded = true;
        } catch (e) { patch({ error: e instanceof Error ? e.message : String(e) }); }
        finally { if (valid()) { lock.current = false; patch({ pending: undefined }); if (succeeded) void refresh(); } }
    }, [client, id, valid, scope, patch, stopping, refresh]);
    return useMemo(() => ({ scope, queue: data?.queue, jobs, loading: !data?.queue && !data?.error,
        error: data?.error, pending: data?.pending, stopping, refresh, onEvent, mutate }),
    [scope, data?.queue, jobs, data?.error, data?.pending, stopping, refresh, onEvent, mutate]);
}
export type SessionActivity = ReturnType<typeof useSessionActivity>;
