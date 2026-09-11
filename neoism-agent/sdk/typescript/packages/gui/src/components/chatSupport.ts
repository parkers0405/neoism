import type { MessageWithParts, Session, SessionRuntimeSnapshot, StepFinishPart, SubagentTask } from "@neoism/sdk";

export function record(value: unknown): Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? value as Record<string, unknown> : {};
}
export const text = (value: unknown): string => typeof value === "string" ? value : "";
export const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
export function finite(value: unknown): number { return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0; }
export function readable(value: unknown): string {
    if (typeof value === "string") return value;
    if (value === undefined || value === null) return "";
    try { return JSON.stringify(value, null, 2) ?? ""; } catch { return "[Unserializable value]"; }
}
export function duration(time: unknown): string {
    const t = record(time);
    if (typeof t.start !== "number" || typeof t.end !== "number" || !Number.isFinite(t.end - t.start) || t.end < t.start) return "";
    const seconds = (t.end - t.start) / 1000;
    return seconds < 1 ? `${Math.round(seconds * 1000)} ms` : seconds < 60 ? `${seconds.toFixed(1)} s` : `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
}
export function reasoningSummary(part: unknown): string {
    const p = record(part), meta = record(p.metadata);
    const summary = text(p.summary) || text(meta.summary) || text(meta.title);
    if (summary.trim()) return summary.trim().slice(0, 180);
    return text(p.text).split("\n").map(line => line.replace(/^[#*>\s]+/, "").trim()).find(Boolean)?.slice(0, 180) || "Reasoning in progress";
}
export function messageAuthor(message: MessageWithParts): string {
    const info = record(message.info);
    const author = text(info.author) || text(record(info.author).name);
    return author.trim() || (info.role === "user" ? "User" : text(info.agent).trim() || "Neoism");
}

/** Validate an event's resource identity even on a supposedly scoped SSE stream. */
export function eventForSession(event: unknown, id: string, knownRequests: ReadonlySet<string> = new Set()): boolean {
    const e = record(event), d = record(e.data), info = record(d.info), subject = record(e.subject);
    const owners = [d.sessionID, d.sessionId, d.parentSessionID, d.parentSessionId, d.rootSessionID,
        info.sessionId, info.sessionID, info.parentId].filter((v): v is string => typeof v === "string");
    if (owners.length) return owners.includes(id);
    if (subject.kind === "session") return subject.id === id;
    return knownRequests.has(text(d.requestID));
}

/** One in-flight request plus one coalesced refresh; never publish a response raced by an event. */
export function refreshQueue<T>(fetch: () => Promise<T>, publish: (value: T) => void, current: () => boolean, onError: (e: unknown) => void) {
    let pending = false, again = false, revision = 0;
    const refresh = async (): Promise<void> => {
        revision++;
        if (!current()) return;
        if (pending) { again = true; return; }
        pending = true;
        do {
            again = false;
            const before = revision;
            try { const value = await fetch(); if (current() && before === revision) publish(value); }
            catch (e) { if (current() && before === revision) onError(e); }
        } while (again && current());
        pending = false;
    };
    return refresh;
}

export type UsageSummary = { cost: number; total: number; input: number; output: number; reasoning: number; cacheRead: number; cacheWrite: number };
export function summarizeUsage(parts: readonly StepFinishPart[]): UsageSummary {
    const result: UsageSummary = { cost: 0, total: 0, input: 0, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0 };
    const seen = new Set<string>();
    for (const part of parts) {
        const key = `${part.sessionId}:${part.messageId}:${part.id}`;
        if (seen.has(key)) continue; seen.add(key);
        const t = record(part.tokens), cache = record(t.cache);
        const input = finite(t.input), output = finite(t.output), read = finite(cache.read), write = finite(cache.write);
        result.cost += finite(part.cost); result.input += input; result.output += output;
        result.reasoning += finite(t.reasoning); result.cacheRead += read; result.cacheWrite += write;
        // Mirrors session_prompt.rs::token_usage_total; reasoning is separately reported, may overlap output.
        result.total += finite(t.total) || input + output + read + write;
    }
    return result;
}

export type TaskRow = { id: string; sessionId: string; title: string; agent: string; status: string; nested: boolean; result?: string; stoppable: boolean };
export const activeTaskStatus = (status: string) => ["running", "pending", "queued", "outstanding", "busy", "retry"].includes(status);
export function taskRows(id: string, tasks: readonly SubagentTask[], children: readonly Session[], runtime?: SessionRuntimeSnapshot): TaskRow[] {
    const rows = new Map<string, TaskRow>();
    for (const child of children) if (child.parentId === id) rows.set(child.id, {
        id: child.id, sessionId: child.id, title: child.title || "Untitled subagent", agent: child.agent || "subagent",
        status: "unknown", nested: false, stoppable: false,
    });
    for (const task of tasks) if (task.sessionId === id && task.childSessionId !== id) rows.set(task.childSessionId, {
        id: task.id, sessionId: task.childSessionId, title: task.description || task.childSessionId,
        agent: task.agent, status: task.status || "unknown", nested: task.nested, result: task.result,
        stoppable: activeTaskStatus(task.status),
    });
    // Branch lifecycle is authoritative for outstanding work, even if the root is idle.
    for (const branch of runtime?.branches || []) {
        if (!branch || typeof branch.sessionId !== "string" || !["outstanding", "completed", "failed"].includes(branch.status)) continue;
        const row = rows.get(branch.sessionId);
        if (!row && branch.parentSessionId !== id) continue;
        // A prior completed branch cannot erase a resumed task or its explicit failure.
        const status = branch.status === "outstanding" ? "outstanding"
            : branch.status === "completed" && row && (row.stoppable || ["error", "failed", "cancelled", "stopped"].includes(row.status)) ? row.status : branch.status;
        rows.set(branch.sessionId, { ...(row || { id: branch.sessionId, sessionId: branch.sessionId,
            title: branch.sessionId, agent: "subagent", nested: false }), status, stoppable: activeTaskStatus(status) });
    }
    return [...rows.values()];
}

export type ScrollSnapshot = { session: string; ids: string[]; height: number; top: number };
export function scrollPlan(previous: ScrollSnapshot | undefined, session: string, ids: string[], height: number, following: boolean): { mode: "reset" | "anchor" | "follow" | "stay"; top?: number } {
    if (!previous || previous.session !== session) return { mode: "reset" };
    const oldFirst = previous.ids[0], index = oldFirst ? ids.indexOf(oldFirst) : -1;
    if (index > 0) return { mode: "anchor", top: previous.top + height - previous.height };
    return { mode: following ? "follow" : "stay" };
}

export type InteractionKeyContext = { key: string; kind: "permission" | "question"; editable?: boolean; button?: boolean; modified?: boolean; composing?: boolean; repeat?: boolean };
export function interactionShortcut(c: InteractionKeyContext): "once" | "always" | "reject" | "submit" | undefined {
    if (c.modified || c.composing || c.repeat || c.editable || (c.button && c.key === "Enter")) return;
    if (c.key === "Escape") return "reject";
    if (c.kind === "question") return c.key === "Enter" ? "submit" : undefined;
    if (c.key === "Enter" || c.key.toLowerCase() === "y") return "once";
    if (c.key.toLowerCase() === "a") return "always";
    if (c.key.toLowerCase() === "n") return "reject";
}
