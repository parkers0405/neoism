import type { MessageWithParts, Part } from "@neoism/sdk";

export type TodoStatus = "pending" | "in_progress" | "completed";
export interface SessionTodo { key: string; content: string; status: TodoStatus; priority?: string; id?: string }
export const todoStatusLabel: Record<TodoStatus, string> = { pending: "Pending", in_progress: "In progress", completed: "Done" };
/** Matches Rust TodoVisualState::from_status (unknown statuses remain pending). */
export function todoStatus(value: unknown): TodoStatus {
    const status = typeof value === "string" ? value.trim().toLowerCase().replaceAll("-", "_") : "";
    if (["completed", "complete", "done", "success"].includes(status)) return "completed";
    if (["in_progress", "active", "running", "current"].includes(status)) return "in_progress";
    return "pending";
}
function record(value: unknown): Record<string, unknown> | undefined {
    return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}
/** Undefined means invalid, [] means an explicit empty plan. Never parse prose. */
export function parseTodos(value: unknown): SessionTodo[] | undefined {
    if (typeof value === "string") { try { value = JSON.parse(value); } catch { return undefined; } }
    if (!Array.isArray(value)) value = record(value)?.todos;
    if (!Array.isArray(value)) return undefined;
    const occurrences = new Map<string, number>();
    const result: SessionTodo[] = [];
    for (const entry of value) {
        const row = record(entry);
        if (!row || typeof row.content !== "string" || !row.content.trim() || typeof row.status !== "string") return undefined;
        const id = typeof row.id === "string" && row.id ? row.id : undefined;
        const base = JSON.stringify(id ? ["id", id] : ["content", row.content]);
        const occurrence = occurrences.get(base) ?? 0; occurrences.set(base, occurrence + 1);
        result.push({ key: `${base}:${occurrence}`, content: row.content, status: todoStatus(row.status),
            ...(id ? { id } : {}), ...(typeof row.priority === "string" ? { priority: row.priority } : {}) });
    }
    return result;
}
export function isTodoTool(part: Part): boolean {
    return part.type === "tool" && ["todowrite", "todo_write", "todo"].includes(part.tool.trim().toLowerCase());
}
/** Only committed output, not a pending/failed tool's proposed input. */
export function todosFromPart(part: Part): SessionTodo[] | undefined {
    if (!isTodoTool(part) || part.type !== "tool" || part.state?.status !== "completed") return undefined;
    return parseTodos(part.state.output) ?? parseTodos(part.state.metadata);
}
export interface TodoSnapshot { todos: SessionTodo[]; partId: string; messageId: string; created: number }
/** Immutable messages may be newest-first API pages or oldest-first GUI state. */
export function latestTodoSnapshot(messages: readonly MessageWithParts[], sessionId: string): TodoSnapshot | undefined {
    let latest: TodoSnapshot | undefined;
    for (const message of messages) {
        if (message.info.sessionId !== sessionId || message.info.role !== "assistant") continue;
        const stamp = message.info.time?.created;
        const created = typeof stamp === "number" && Number.isFinite(stamp) ? stamp : 0;
        for (const part of message.parts) {
            if (part.sessionId !== sessionId) continue;
            const todos = todosFromPart(part);
            if (todos === undefined) continue;
            // IDs are chronological server identifiers, used only for equal/missing time.
            if (!latest || created > latest.created || created === latest.created && message.info.id >= latest.messageId)
                latest = { todos, partId: part.id, messageId: message.info.id, created };
        }
    }
    return latest;
}
/** Preserve array and row references across token-only message updates. */
export function reconcileTodos(previous: readonly SessionTodo[], next: readonly SessionTodo[]): readonly SessionTodo[] {
    const byKey = new Map(previous.map(todo => [todo.key, todo]));
    const result = next.map(todo => {
        const old = byKey.get(todo.key);
        return old && old.content === todo.content && old.status === todo.status && old.priority === todo.priority ? old : todo;
    });
    return result.length === previous.length && result.every((todo, i) => todo === previous[i]) ? previous : result;
}
