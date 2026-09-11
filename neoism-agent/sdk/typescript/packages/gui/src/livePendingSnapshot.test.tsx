import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Event, MessageWithParts, Part, SessionRuntimeSnapshot } from "@neoism/sdk";
import { livePartLedger, markLiveEvent, outstandingTaskParts, pendingSnapshotMessage, visibleDetailParts } from "./livePartOrigins";
import { Timeline } from "./components/Timeline";
const task = (id: string, status: string, metadata: unknown = {}): Part => ({ type: "tool", id, messageId: "current", sessionId: "s", tool: "functions.task", callId: id, state: { status, input: { description: id }, metadata } } as Part);
const message = (id: string, parts: Part[], completed?: number): MessageWithParts => ({ info: { id, sessionId: "s", role: "assistant", time: { created: 1, completed } }, parts } as MessageWithParts);
const render = (messages: MessageWithParts[], busy: boolean, runtime?: SessionRuntimeSnapshot) => renderToStaticMarkup(<Timeline messages={messages} busy={busy} sessionId="s" loading={false} loadOlder={() => {}} runtime={runtime} />);
it("recovers pending/running tools only on the latest unfinished assistant snapshot while busy", () => {
    const old = message("old", [task("OLD", "running")]);
    const current = message("current", [task("PENDING", "pending"), task("RUNNING", "running"), task("COMPLETED", "completed")]);
    expect(pendingSnapshotMessage([old, current], true)).toBe("current");
    const html = render([old, current], true);
    expect(html).toContain("Task(PENDING)"); expect(html).toContain("Task(RUNNING)"); expect(html).not.toContain("Task(OLD)"); expect(html).not.toContain("Task(COMPLETED)");
    expect(render([old, current], false)).not.toContain("Task(");
    expect(render([old, message("done", [task("ORPHAN", "pending")], 3)], true)).not.toContain("Task(");
    const finished = { ...current, info: { ...current.info, finish: "stop" } } as MessageWithParts;
    expect(pendingSnapshotMessage([old, finished], true)).toBeUndefined();
    const prompt = { info: { id: "new", sessionId: "s", role: "user", time: {} }, parts: [] } as MessageWithParts;
    expect(pendingSnapshotMessage([old, prompt], true)).toBeUndefined();
});
it("recovers only actively timed reasoning, not ended reasoning or unstarted subtask instructions", () => {
    const parts = [
        { id: "live", type: "reasoning", time: { start: 1 }, text: "LIVE" },
        { id: "ended", type: "reasoning", time: { start: 1, end: 2 }, text: "ENDED" },
        { id: "subtask", type: "subtask", description: "scheduled" },
    ] as Part[];
    expect(visibleDetailParts(parts, undefined, true).map(p => p.id)).toEqual(["live"]);
});
it("keeps late SSE parts visible after completion without exposing historical siblings", () => {
    const ledger = livePartLedger(), part = task("NEW", "running");
    markLiveEvent(ledger, { type: "message.part.updated", data: { part } } as Event);
    const completed = task("NEW", "completed"), m = message("current", [completed, task("OLD", "completed")], 3);
    const html = renderToStaticMarkup(<Timeline messages={[m]} busy={false} loading={false} loadOlder={() => {}} liveParts={ledger.parts} />);
    expect(html).toContain("Task(NEW)"); expect(html).not.toContain("Task(OLD)");
});
it("uses current outstanding runtime child IDs, never static metadata alone, to recover background tasks", () => {
    const rows = [message("historical", [task("OLD", "completed", { sessionId: "old-child", status: "running" })], 2),
        message("background", [task("ACTIVE", "completed", { sessionId: "child", status: "running" })], 3)];
    const runtime: SessionRuntimeSnapshot = { rootSessionId: "s", revision: 1, branches: [{ parentSessionId: "s", sessionId: "child", status: "outstanding" }] };
    expect(outstandingTaskParts(rows, runtime, "s").get("background")).toBe('["ACTIVE"]');
    const html = render(rows, false, runtime);
    expect(html).toContain("Task(ACTIVE)"); expect(html).not.toContain("Task(OLD)");
    expect(render(rows, false, { ...runtime, rootSessionId: "other" })).not.toContain("Task(");
    expect(render(rows, false, { ...runtime, branches: [{ ...runtime.branches[0], status: "completed" }] })).not.toContain("Task(");
});
