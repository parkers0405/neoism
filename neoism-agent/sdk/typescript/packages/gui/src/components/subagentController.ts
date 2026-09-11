import { subscribeGuiEvents } from "../sharedEvents";
import type { NeoismClient, Session, SessionRuntimeSnapshot, SubagentTask } from "@neoism/sdk";
import { errorMessage } from "../types";
import { eventForSession, refreshQueue, taskRows, type TaskRow } from "./chatSupport";

export type SubagentSnapshot = { rows: TaskRow[]; loading: boolean; canStop: boolean; stopping?: string; errors: string[]; notice: string };
export const emptySubagents = (): SubagentSnapshot => ({ rows: [], loading: true, canStop: false, errors: [], notice: "" });
export async function fetchSubagents(client: NeoismClient, id: string, signal: AbortSignal) {
    const path = { session_id: id };
    const results = await Promise.allSettled([
        client.operations.request("v2.subagents.tasks.list", { path, signal }),
        client.operations.request("v2.sessions.children", { path, signal }),
        client.operations.request("v2.sessions.runtime", { path, signal }),
    ]);
    const errors: string[] = [];
    const labels = ["Task controls", "Child sessions", "Runtime status"];
    results.forEach((r, i) => { if (r.status === "rejected") errors.push(`${labels[i]}: ${errorMessage(r.reason)}`); });
    const [tasksResult, childResult, runtimeResult] = results;
    const tasks: SubagentTask[] = tasksResult.status === "fulfilled" && Array.isArray(tasksResult.value) ? tasksResult.value.filter(t => t && typeof t.id === "string" && typeof t.childSessionId === "string") : [];
    const children: Session[] = childResult.status === "fulfilled" && Array.isArray(childResult.value?.items) ? childResult.value.items.filter(s => s && typeof s.id === "string") : [];
    const runtime: SessionRuntimeSnapshot | undefined = runtimeResult.status === "fulfilled" && Array.isArray(runtimeResult.value?.branches) ? runtimeResult.value : undefined;
    return { rows: taskRows(id, tasks, children, runtime), canStop: tasksResult.status === "fulfilled", errors };
}

export function createSubagentController(client: NeoismClient, id: string, isCurrent: () => boolean,
    publish: (snapshot: SubagentSnapshot) => void) {
    const abort = new AbortController(), signal = abort.signal;
    let state = emptySubagents();
    const current = () => !signal.aborted && isCurrent();
    const emit = () => { if (current()) publish({ ...state }); };
    const refresh = refreshQueue(() => fetchSubagents(client, id, signal), snapshot => {
        state = { ...state, ...snapshot, loading: false }; emit();
    }, current, e => { state = { ...state, loading: false, errors: [errorMessage(e)] }; emit(); });
    async function stop(taskId?: string) {
        if (!current() || state.stopping || !state.canStop || !state.rows.some(r => r.stoppable && (!taskId || r.id === taskId))) return false;
        state = { ...state, stopping: taskId || "*", notice: "" }; emit();
        try {
            if (!current()) return false;
            const result = await client.operations.request("v2.subagents.tasks.stop", { path: { session_id: id }, body: taskId ? { taskId } : {}, signal });
            if (!current()) return false;
            state = { ...state, notice: result.stopped.length
                ? `Stopped ${result.stopped.length} task(s); cleared ${result.clearedPrompts} queued prompt(s).`
                : `No running tasks were stopped; cleared ${result.clearedPrompts} queued prompt(s).` };
            return true;
        } catch (e) { if (current()) state = { ...state, notice: `Could not stop task: ${errorMessage(e)}` }; return false; }
        finally { if (current()) { state = { ...state, stopping: undefined }; emit(); void refresh(); } }
    }
    async function events() {
        try {
            for await (const event of subscribeGuiEvents(client, { sessionId: id, tail: true, signal })) {
                if (!current()) break;
                const known = state.rows.map(row => row.sessionId);
                if (!eventForSession(event, id) && !known.some(child => eventForSession(event, child))) continue;
                if (["session.status", "session.created", "session.updated", "session.deleted", "session.execution.updated", "session.subtask.completed"].includes(event.type)) void refresh();
            }
        } catch (e) {
            if (current()) { state = { ...state, errors: [...state.errors, `Live task updates: ${errorMessage(e)}. Periodic refresh remains active.`] }; emit(); }
        }
    }
    return { refresh, stop, events, dispose: () => abort.abort() };
}
export type SubagentController = ReturnType<typeof createSubagentController>;
