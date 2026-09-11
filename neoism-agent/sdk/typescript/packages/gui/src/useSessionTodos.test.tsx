// @vitest-environment happy-dom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { Event as SdkEvent, MessageWithParts, NeoismClient } from "@neoism/sdk";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useSessionTodos } from "./useSessionTodos";
const pending = [{ content: "ship runtime", status: "pending", priority: "high" }];
const done = [{ ...pending[0], status: "completed" }];
const event = (sessionID: string, todos = done, sequence = 1): SdkEvent => ({ type: "todo.updated", data: { sessionID, todos }, sequence, source: "server", id: `e${sequence}`, timestamp: 1, schemaVersion: "2" });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
let root: Root, el: HTMLDivElement, result: ReturnType<typeof useSessionTodos>;
function Harness({ client, id = "s", messages = [], serverKey = "one" }: { client: NeoismClient; id?: string; messages?: MessageWithParts[]; serverKey?: string }) {
    result = useSessionTodos(client, id, { messages, serverKey, subscribe: false });
    return <div>{result.todos.map(t => `${t.content}:${t.status}`).join(",")}</div>;
}
function clientFor(request: ReturnType<typeof vi.fn>) { return { operations: { request }, events: { subscribe: vi.fn() } } as unknown as NeoismClient; }
beforeEach(() => { (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true; el = document.createElement("div"); document.body.append(el); root = createRoot(el); });
afterEach(() => { act(() => root.unmount()); el.remove(); });
it("fetches once per attach, ignores token/foreign events, and live updates beat a stale fetch", async () => {
    const response = deferred<unknown>(), request = vi.fn((_operation: string, _input: { signal: AbortSignal }) => response.promise), client = clientFor(request);
    await act(async () => root.render(<Harness client={client} />));
    expect(request).toHaveBeenCalledTimes(1); expect(request.mock.calls[0][0]).toBe("v2.sessions.todos");
    act(() => result.onEvent(event("other"))); expect(result.todos).toHaveLength(0);
    act(() => result.onEvent({ type: "message.part.delta", data: { sessionID: "s" } } as SdkEvent));
    act(() => result.onEvent(event("s"))); expect(result.todos[0].status).toBe("completed");
    await act(async () => response.resolve(pending)); expect(result.todos[0].status).toBe("completed");
    act(() => result.onEvent(event("s", pending, 0))); expect(result.todos[0].status).toBe("completed");
    await act(async () => root.render(<Harness client={client} messages={[]} />)); expect(request).toHaveBeenCalledTimes(1);
    act(() => result.onEvent(event("s", [], 2))); expect(result.todos).toHaveLength(0);
});
it("guards same-session client changes, server scopes, old callbacks and in-flight responses", async () => {
    const response = deferred<unknown>(), firstRequest = vi.fn((_operation: string, _input: { signal: AbortSignal }) => response.promise), first = clientFor(firstRequest);
    const secondRequest = vi.fn().mockResolvedValue(pending), second = clientFor(secondRequest);
    await act(async () => root.render(<Harness client={first} />)); const oldEvent = result.onEvent;
    await act(async () => root.render(<Harness client={second} />));
    await act(async () => { oldEvent(event("s")); response.resolve(done); });
    expect(result.todos[0].status).toBe("pending");
    const previousServerEvent = result.onEvent;
    await act(async () => root.render(<Harness client={second} serverKey="two" />));
    act(() => previousServerEvent(event("s"))); expect(result.todos[0].status).toBe("pending");
    expect(secondRequest).toHaveBeenCalledTimes(2);
    await act(async () => root.render(<Harness client={second} id="other" serverKey="two" />));
    act(() => result.onEvent(event("s"))); expect(result.todos[0].status).toBe("pending");
    expect(firstRequest.mock.calls[0][1].signal.aborted).toBe(true);
});
it("uses latest immutable tool snapshot for unsupported endpoints without per-token requests", async () => {
    const request = vi.fn().mockRejectedValue({ status: 404 }), client = clientFor(request);
    const message = (created: number, todos: typeof pending): MessageWithParts => ({ info: { id: `m${created}`, sessionId: "s", role: "assistant", time: { created } }, parts: [{ type: "tool", id: `p${created}`, sessionId: "s", messageId: `m${created}`, callId: "c", tool: "todowrite", state: { status: "completed", input: {}, output: JSON.stringify(todos), metadata: { todos }, title: "tasks", time: { start: 0 } } }] });
    await act(async () => root.render(<Harness client={client} messages={[message(1, pending)]} />));
    expect(result.source).toBe("messages"); expect(result.error).toBeUndefined(); const key = result.todos[0].key;
    await act(async () => root.render(<Harness client={client} messages={[message(3, done), message(1, pending)]} />));
    expect(result.todos[0].status).toBe("completed"); expect(result.todos[0].key).toBe(key); expect(result.todos).toHaveLength(1);
    const stable = result.todos;
    await act(async () => root.render(<Harness client={client} messages={[message(3, done), message(1, pending)]} />));
    expect(result.todos).toBe(stable); expect(request).toHaveBeenCalledTimes(1);
});
it("survives StrictMode cleanup/setup and rejects malformed live snapshots", async () => {
    const request = vi.fn().mockResolvedValue(pending), client = clientFor(request);
    await act(async () => root.render(<StrictMode><Harness client={client} /></StrictMode>));
    act(() => result.onEvent(event("s", [null] as unknown as typeof done)));
    expect(result.todos[0].status).toBe("pending");
    act(() => result.onEvent(event("s"))); expect(result.todos[0].status).toBe("completed");
});
it("subscribes to the scoped SDK stream and aborts both stream and request on detach", async () => {
    const incoming = deferred<SdkEvent>(), request = vi.fn().mockResolvedValue(pending);
    let signal: AbortSignal | undefined;
    const subscribe = vi.fn(async function* (options: { signal: AbortSignal; sessionId: string }) {
        signal = options.signal;
        yield await incoming.promise;
    });
    const client = { operations: { request }, events: { subscribe } } as unknown as NeoismClient;
    function Streaming() { result = useSessionTodos(client, "s"); return null; }
    await act(async () => root.render(<Streaming />));
    expect(subscribe).toHaveBeenCalledWith(expect.objectContaining({ sessionId: "s", tail: true }));
    await act(async () => incoming.resolve(event("s")));
    expect(result.todos[0].status).toBe("completed"); expect(request).toHaveBeenCalledTimes(1);
    await act(async () => root.render(null)); expect(signal?.aborted).toBe(true);
});
