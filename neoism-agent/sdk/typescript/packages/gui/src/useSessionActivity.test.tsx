// @vitest-environment happy-dom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createNeoismClient, type Event, type NeoismClient, type SessionQueueInfo, type SessionRuntimeSnapshot } from "@neoism/sdk";
import { useChat } from "./useChat";
import { useSessionActivity, type SessionActivity } from "./useSessionActivity";
import { NativeActivity } from "./components/nativeActivity";
let root: Root, container: HTMLDivElement, activity: SessionActivity;
const queue = (count: number, sessionId = "root"): SessionQueueInfo => ({ count, sessionId, running: false, worker: false, items: Array.from({ length: count }, (_, index) => ({ index, text: `Prompt ${index}`, noReply: false, partCount: 1 })) });
const runtime: SessionRuntimeSnapshot = { revision: 1, rootSessionId: "root", branches: [], runningBackgroundTasks: [{ jobId: "job-1", sessionId: "child", startedAt: Date.now() - 3000 }] };
function Harness({ client, id = "root", jobs = false }: { client: NeoismClient; id?: string; jobs?: boolean }) {
    activity = useSessionActivity(client, id, jobs ? runtime : undefined);
    return <NativeActivity busy={false} sessionId={id} activity={{ status: "idle" }} sessionActivity={activity} />;
}
function sdk(request: (r: any) => Promise<any>) { return createNeoismClient({ request, async *events() {} }); }
beforeEach(() => { (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true; container = document.createElement("div"); document.body.append(container); root = createRoot(container); });
afterEach(() => { act(() => root.unmount()); container.remove(); vi.restoreAllMocks(); });
async function render(client: NeoismClient, id = "root", jobs = false) { await act(async () => root.render(<Harness client={client} id={id} jobs={jobs} />)); }
function click(selector: string) { act(() => (document.querySelector(selector) as HTMLElement).click()); }
it("uses SDK queue snapshots while idle, applies delivery/pop/clear events without delta fetches", async () => {
    const request = vi.fn(async () => queue(2)), client = sdk(request); await render(client);
    expect(container.textContent).toContain("queued messages (2)"); expect(container.querySelector("canvas")).toBeNull();
    for (const action of ["delivery", "pop", "clear"]) {
        act(() => activity.onEvent({ type: "session.queue.updated", data: { sessionID: "root", action, queue: queue(action === "clear" ? 0 : 1) } } as Event));
        expect(activity.queue?.count).toBe(action === "clear" ? 0 : 1);
    }
    for (let i = 0; i < 100; i++) act(() => activity.onEvent({ type: "message.part.delta", data: {} } as Event));
    expect(request).toHaveBeenCalledTimes(1); expect(container.textContent).toBe("");
});
it("stops only the owning branch job through the generated DELETE endpoint", async () => {
    const request = vi.fn(async (r: any) => r.method === "DELETE" ? { status: "stopping", jobId: "job-1" } : queue(0));
    await render(sdk(request), "root", true); click(".native-activity-background");
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    await act(async () => (document.querySelector('[aria-label="Stop job job-1"]') as HTMLElement).click());
    expect(request.mock.calls.some(([r]) => r.method === "DELETE" && r.path === "/v2/sessions/child/jobs/job-1")).toBe(true);
    expect(request.mock.calls.some(([r]) => r.path.includes("abort"))).toBe(false);
    expect(document.querySelector('[aria-label="Stop job job-1"]')?.textContent).toBe("Stopping…");
    act(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(document.querySelector('[role="dialog"]')).toBeNull(); expect(document.activeElement).toBe(container.querySelector("button"));
});
it("queue rows expose previews and only real remove-next / clear actions", async () => {
    let count = 2;
    const request = vi.fn(async (r: any) => { if (r.method === "POST") return { removed: 1, queue: queue(--count) }; if (r.method === "DELETE") return { removed: count, queue: queue(count = 0) }; return queue(count); });
    await render(sdk(request)); await act(async () => (container.querySelector("button") as HTMLElement).click());
    const summary = document.querySelector(".activity-popover summary") as HTMLElement;
    expect(summary.textContent).toContain("Prompt 0");
    act(() => summary.click()); expect(summary.parentElement?.hasAttribute("open")).toBe(true);
    const actionButton = (text: string) => [...document.querySelectorAll<HTMLButtonElement>(".activity-popover button")].find(button => button.textContent === text)!;
    await act(async () => actionButton("Remove next").click()); expect(activity.queue?.count).toBe(1);
    await act(async () => actionButton("Clear queue").click()); expect(activity.queue?.count).toBe(0);
    expect(request.mock.calls.some(([r]) => r.method === "POST" && r.path.endsWith("/queue/pop"))).toBe(true);
    expect(request.mock.calls.some(([r]) => r.method === "DELETE" && r.path.endsWith("/queue"))).toBe(true);
    act(() => document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
});
it("aborts old fetches, dismisses portals and denies old actions/results after a scope switch", async () => {
    let finish!: (value: any) => void;
    const request = vi.fn((r: any) => r.method === "DELETE" ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(queue(1)));
    const client = sdk(request); await render(client); await act(async () => (container.querySelector("button") as HTMLElement).click());
    const old = activity; let mutation!: Promise<void>;
    act(() => { mutation = old.mutate("clear"); });
    const other = sdk(async () => queue(2, "other")); await render(other, "other");
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(old.scope.controller.signal.aborted).toBe(true);
    await act(async () => { finish({ queue: queue(0) }); await mutation; await old.mutate("pop"); });
    expect(activity.queue?.count).toBe(2); expect(request.mock.calls.filter(([r]) => r.method === "POST")).toHaveLength(0);
});
it("survives StrictMode effect replay without accepting the aborted initial request", async () => {
    const pending: { resolve(value: any): void; signal: AbortSignal }[] = [];
    const client = sdk(r => new Promise(resolve => pending.push({ resolve, signal: r.signal })));
    await act(async () => root.render(<StrictMode><Harness client={client} /></StrictMode>));
    expect(pending).toHaveLength(2); expect(pending[0].signal.aborted).toBe(true);
    await act(async () => pending[1].resolve(queue(2)));
    await act(async () => pending[0].resolve(queue(7)));
    expect(activity.queue?.count).toBe(2);
});
it("keeps stop failures visible and permits retry instead of stopping the parent", async () => {
    const client = sdk(async r => { if (r.method === "DELETE") throw new Error("Job stop denied"); return queue(0); });
    await render(client, "root", true);
    await act(async () => (container.querySelector("button") as HTMLElement).click());
    await act(async () => (document.querySelector('[aria-label="Stop job job-1"]') as HTMLElement).click());
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Job stop denied");
    expect((document.querySelector('[aria-label="Stop job job-1"]') as HTMLButtonElement).disabled).toBe(false);
});
it("integrates queue/runtime fetches with the one existing chat event subscription", async () => {
    const events = vi.fn(async function* ({ signal }: { signal?: AbortSignal } = {}) { await new Promise<void>(resolve => signal?.addEventListener("abort", () => resolve(), { once: true })); });
    const request = vi.fn(async (r: any) => r.path.endsWith("/queue") ? queue(2) : r.path.endsWith("/runtime") ? runtime : r.path.endsWith("/messages") ? { items: [], cursor: {} } : {});
    const client = createNeoismClient({ request: request as NeoismClient["transport"]["request"], events });
    const notify = vi.fn();
    function ChatHost() { const chat = useChat(client, "root", notify); return <NativeActivity busy={false} activity={{ status: "idle" }} sessionActivity={chat.sessionActivity} />; }
    await act(async () => root.render(<ChatHost />));
    expect(events).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("queued messages (2)"); expect(container.textContent).toContain("1 background task running");
    expect(request.mock.calls.some(([r]) => r.path === "/v2/sessions/root/runtime")).toBe(true);
    expect(notify).not.toHaveBeenCalled();
});
it("SSE queue snapshot wins over a late list fetch", async () => {
    let finish!: (value: any) => void;
    await render(sdk(() => new Promise(resolve => { finish = resolve; })));
    act(() => activity.onEvent({ type: "session.queue.updated", data: { sessionID: "root", queue: queue(3) } } as Event));
    await act(async () => finish(queue(1))); expect(activity.queue?.count).toBe(3);
});
