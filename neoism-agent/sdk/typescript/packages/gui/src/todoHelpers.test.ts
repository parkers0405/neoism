import { describe, expect, it } from "vitest";
import type { MessageWithParts, Part } from "@neoism/sdk";
import { latestTodoSnapshot, parseTodos, reconcileTodos, todosFromPart, todoStatus } from "./todoHelpers";

// tool_runtime.rs todowrite: full JSON output plus metadata.todos; no IDs required.
export const rustTodos = [
    { content: "ship runtime", status: "in_progress", priority: "high" },
    { content: "write tests", status: "pending", priority: "medium" },
];
export function todoPart(todos: unknown = rustTodos, sessionId = "s", id = "p"): Part {
    return { type: "tool", tool: "todowrite", id, callId: id, messageId: "m", sessionId,
        state: { status: "completed", input: { todos }, metadata: { todos }, output: JSON.stringify(todos), title: "2 todos", time: { start: 1, end: 2 } } } as Part;
}
export function todoMessage(created: number, todos: unknown, id = `m${created}`, sessionId = "s"): MessageWithParts {
    return { info: { id, sessionId, role: "assistant", time: { created } }, parts: [todoPart(todos, sessionId, `p${created}`)] };
}
describe("native task payloads", () => {
    it("accepts real tool output and metadata, not text checklists or unfinished proposals", () => {
        expect(todosFromPart(todoPart())).toEqual(parseTodos(rustTodos));
        const part = todoPart();
        if (part.type !== "tool") throw Error();
        expect(todosFromPart({ ...part, state: { ...part.state, output: "2 todos" } })).toEqual(parseTodos(rustTodos));
        for (const status of ["pending", "running", "error"]) expect(todosFromPart({ ...part, state: { ...part.state, status } } as Part)).toBeUndefined();
        expect(parseTodos("- [ ] ship runtime")).toBeUndefined();
    });
    it("matches all native aliases", () => {
        for (const alias of ["completed", "complete", "done", "success", " DONE "]) expect(todoStatus(alias)).toBe("completed");
        for (const alias of ["in_progress", "in-progress", "active", "running", "current"]) expect(todoStatus(alias)).toBe("in_progress");
        for (const alias of ["pending", "unknown", "cancelled", null]) expect(todoStatus(alias)).toBe("pending");
    });
    it("rejects malformed snapshots atomically and recognizes explicit clears", () => {
        for (const value of [null, {}, "bad json", [null], [{ content: "", status: "done" }], [{ content: "x" }], [...rustTodos, { content: 5, status: "done" }]]) expect(parseTodos(value)).toBeUndefined();
        expect(parseTodos([])).toEqual([]); expect(parseTodos({ todos: rustTodos })).toHaveLength(2);
    });
    it("keys IDs or content occurrences independently of status, priority and position", () => {
        const before = parseTodos([{ id: "a", content: "old", status: "pending" }, ...rustTodos, rustTodos[0]])!;
        const after = parseTodos([rustTodos[1], { id: "a", content: "renamed", status: "done" }, { ...rustTodos[0], status: "done" }, rustTodos[0]])!;
        expect(after.map(x => x.key)).toEqual([before[2].key, before[0].key, before[1].key, before[3].key]);
        expect(new Set(before.map(x => x.key)).size).toBe(4);
        expect(reconcileTodos(before, parseTodos([{ id: "a", content: "old", status: "pending" }, ...rustTodos, rustTodos[0]])!)).toBe(before);
        const changed = reconcileTodos(before, after); expect(changed[0]).toBe(before[2]); expect(changed[2].status).toBe("completed");
    });
    it("selects only latest session assistant snapshot regardless of page order; never replays lists", () => {
        const old = todoMessage(1, rustTodos), latest = todoMessage(3, [{ ...rustTodos[0], status: "done" }]);
        const foreign = todoMessage(99, rustTodos, "foreign", "other");
        const user = { ...todoMessage(100, rustTodos), info: { ...old.info, role: "user" as const } };
        expect(latestTodoSnapshot([latest, foreign, old, user], "s")?.partId).toBe("p3");
        expect(latestTodoSnapshot([old, latest], "s")?.todos[0].status).toBe("completed");
        expect(latestTodoSnapshot([latest, todoMessage(4, [])], "s")?.todos).toEqual([]);
        expect(latestTodoSnapshot([latest, todoMessage(4, null)], "s")?.partId).toBe("p3");
    });
});
