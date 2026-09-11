import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { MessageWithParts, NeoismClient, Part, Session, SessionRuntimeSnapshot, StepFinishPart, SubagentTask } from "@neoism/sdk";
import { duration, eventForSession, interactionShortcut, messageAuthor, refreshQueue, scrollPlan, summarizeUsage, taskRows } from "./chatSupport";
import { createInteractionController, decodePermissions, decodeQuestions, emptyInteractions, questionAnswers, type InteractionSnapshot } from "./interactionController";
import { createSubagentController, emptySubagents, fetchSubagents, type SubagentSnapshot } from "./subagentController";
import { PartView, Timeline, presentAssistantParts } from "./Timeline";
import { QuestionCard } from "./Interactions";
import { ChatDetails } from "./ChatDetails";
import type { useAppController } from "../useAppController";

const deferred = <T,>() => {
    let resolve!: (value: T) => void, reject!: (error: unknown) => void;
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
};
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const permission = (id = "p", sessionId = "s") => ({ id, sessionId, permission: "bash", title: "Run tests", patterns: ["npm test"], always: [], messageId: "m" });
const question = (questions: unknown[] = [{ question: "Choose", options: [{ label: "Yes" }, { label: "No" }] }]) => ({ id: "q", sessionId: "s", messageId: "m", questions });
function mockClient(handler: (op: string, input: any) => unknown, events: unknown[] = []) {
    const request = vi.fn(async (op: string, input: unknown) => handler(op, input));
    const client = { operations: { request }, events: { subscribe: async function* () { for (const event of events) yield event; } } } as unknown as NeoismClient;
    return { client, request };
}
const message = (id: string, sessionId = "s", author?: string): MessageWithParts => ({ info: { id, sessionId, role: "user", time: {}, author }, parts: [] });
const task = (status = "running"): SubagentTask => ({ id: "t", sessionId: "s", childSessionId: "child", agent: "explore", description: "Read sources", status, nested: false });
const runtime = (status: "outstanding" | "completed" | "failed" = "outstanding"): SessionRuntimeSnapshot => ({ rootSessionId: "s", revision: 1, branches: [{ parentSessionId: "s", sessionId: "child", status }] });
const child = { id: "child", parentId: "s", title: "Child", agent: "explore" } as Session;

describe("assistant thinking/answer presentation", () => {
    it("paints seeded text after reasoning so answer tokens cannot jump above thinking", () => {
        expect(presentAssistantParts([
            {type:"text",id:"answer",text:"Hello"},
            {type:"reasoning",id:"think",text:"plan",time:{start:2}},
        ]).map(part => part.id)).toEqual(["think","answer"]);
    });
    it("keeps a finished answer above later reasoning", () => {
        expect(presentAssistantParts([
            {type:"text",id:"answer",text:"Done.",time:{start:1,end:2}},
            {type:"reasoning",id:"think",text:"next",time:{start:3}},
        ]).map(part => part.id)).toEqual(["answer","think"]);
    });
    it("leaves chronological text after thinking in place", () => {
        expect(presentAssistantParts([
            {type:"reasoning",id:"think",text:"plan"},
            {type:"text",id:"answer",text:"Hello"},
        ]).map(part => part.id)).toEqual(["think","answer"]);
    });
});
describe("safe interaction decoding and answer validation", () => {
    it("filters other sessions and malformed/duplicate permission rows", () => {
        expect(decodePermissions([null, {}, permission(), permission(), permission("other", "elsewhere")], "s")).toHaveLength(1);
        expect(decodeQuestions([null, {}, question(), { ...question(), sessionId: "elsewhere" }], "s")).toHaveLength(1);
    });
    it("never crashes on unknown question JSON and keeps malformed requests rejectable", () => {
        const [q] = decodeQuestions([question([null, "text", { question: "Options", options: [null, 1, { label: 2 }, { label: "Yes" }, { label: "Yes" }] }])], "s");
        expect(q.questions.map(q => q.valid)).toEqual([false, false, true]);
        expect(q.questions[2].options).toEqual([{ label: "Yes", description: "" }]);
        expect(questionAnswers(q, [])).toBeUndefined();
        const html = renderToStaticMarkup(<QuestionCard question={q} busy={false} submit={() => {}} reject={() => {}} />);
        expect(html).toContain("unsupported question"); expect(html).toContain("Reject"); expect(html).toContain('type="submit" class="primary" disabled');
    });
    it("requires every answer, trims free text, and preserves multi-select alongside custom", () => {
        const [q] = decodeQuestions([question([{ question: "Multi", multiple: true, options: [{ label: "A" }, { label: "B" }] }, { question: "Why?" }])], "s");
        expect(questionAnswers(q, [{ selected: ["A"], custom: " " }, { selected: [], custom: "" }])).toBeUndefined();
        expect(questionAnswers(q, [{ selected: ["A", "A", "made up"], custom: " extra " }, { selected: [], custom: " because " }])).toEqual([["A", "extra"], ["because"]]);
        expect(questionAnswers({ ...q, questions: [] }, [])).toBeUndefined();
    });
    it("respects custom:false and single-select cardinality", () => {
        const [q] = decodeQuestions([question([{ question: "Pick", custom: false, options: [{ label: "A" }, { label: "B" }] }])], "s");
        expect(questionAnswers(q, [{ selected: [], custom: "injected" }])).toBeUndefined();
        expect(questionAnswers(q, [{ selected: ["A", "B"], custom: "" }])).toBeUndefined();
        expect(questionAnswers(q, [{ selected: ["A"], custom: "ignored" }])).toEqual([["A"]]);
    });
    it("maps focused-card shortcuts, but never consumes typing, IME, modifier keys, or button Enter", () => {
        for (const [key, result] of [["y", "once"], ["a", "always"], ["n", "reject"], ["Enter", "once"], ["Escape", "reject"]])
            expect(interactionShortcut({ key, kind: "permission" })).toBe(result);
        expect(interactionShortcut({ key: "Enter", kind: "question" })).toBe("submit");
        expect(interactionShortcut({ key: "Escape", kind: "question" })).toBe("reject");
        expect(interactionShortcut({ key: "y", kind: "question" })).toBeUndefined();
        for (const flag of ["editable", "button", "modified", "composing", "repeat"])
            expect(interactionShortcut({ key: "Enter", kind: "permission", [flag]: true })).toBeUndefined();
    });
});

describe("session-scoped async interaction lifecycle", () => {
    it("does not auto-approve or publish an old session's late list response", async () => {
        const p = deferred<unknown>(); let current = true;
        const { client, request } = mockClient(op => op.endsWith("permissions.list") ? p.promise : []);
        const publish = vi.fn(), report = vi.fn();
        const store = createInteractionController(client, "s", true, () => current, publish, report);
        const pending = store.refresh(); current = false; p.resolve([permission()]); await pending;
        expect(publish).not.toHaveBeenCalled(); expect(report).not.toHaveBeenCalled();
        expect(request.mock.calls.some(([op]) => op.endsWith("reply"))).toBe(false); store.dispose();
    });
    it("rechecks session ownership between auto-approvals and excludes forwarded children", async () => {
        let current = true;
        const { client, request } = mockClient(op => {
            if (op.endsWith("permissions.list")) return [permission("one"), permission("two"), { ...permission("forwarded", "child"), parentSessionID: "s" }];
            if (op.endsWith("permissions.reply")) { current = false; return true; }
            return [];
        });
        const store = createInteractionController(client, "s", true, () => current, () => {}, () => {});
        await store.refresh(); await flush();
        expect(request.mock.calls.filter(([op]) => op.endsWith("permissions.reply"))).toHaveLength(1); store.dispose();
        const forwarded = mockClient(op => op.endsWith("permissions.list") ? [{ ...permission("forwarded", "child"), parentSessionID: "s" }] : []);
        const second = createInteractionController(forwarded.client, "s", true, () => true, () => {}, () => {});
        await second.refresh(); await flush();
        expect(forwarded.request.mock.calls.some(([op]) => op.endsWith("reply"))).toBe(false); second.dispose();
    });
    it("ignores stale action success/error/finally and prevents duplicate submissions", async () => {
        const reply = deferred<boolean>(); let current = true;
        const { client, request } = mockClient(op => op.endsWith("permissions.list") ? [permission()] : op.endsWith("reply") ? reply.promise : []);
        const publish = vi.fn(), report = vi.fn();
        const store = createInteractionController(client, "s", false, () => current, publish, report);
        await store.refresh(); const pending = store.permission("p", "once");
        expect(await store.permission("p", "once")).toBe(false);
        const count = publish.mock.calls.length; current = false; reply.reject(new Error("Old session failed")); await pending;
        expect(publish).toHaveBeenCalledTimes(count); expect(report).not.toHaveBeenCalled();
        expect(request.mock.calls.filter(([op]) => op.endsWith("reply"))).toHaveLength(1); store.dispose();
    });
    it("keeps rejected server responses visible, then tombstones successful replies across stale polls", async () => {
        let accepted = false, snapshot = emptyInteractions(); const report = vi.fn();
        const { client } = mockClient(op => op.endsWith("permissions.list") ? [permission()] : op.endsWith("reply") ? accepted : []);
        const store = createInteractionController(client, "s", false, () => true, s => { snapshot = s; }, report);
        await store.refresh(); expect(await store.permission("p", "once")).toBe(false); await flush();
        expect(snapshot.permissions).toHaveLength(1); expect(report).toHaveBeenCalled();
        accepted = true; await store.permission("p", "once"); await store.refresh(); await flush();
        expect(snapshot.permissions).toHaveLength(0); store.dispose();
    });
    it("refreshes immediately on scoped SSE, removes replies immediately, and ignores other sessions", async () => {
        let permissions: unknown[] = [permission()], snapshot = emptyInteractions();
        const events = [{ type: "permission.asked", data: permission("new") }, { type: "permission.replied", data: { requestID: "p" } }, { type: "permission.asked", data: permission("else", "other") }];
        const { client, request } = mockClient(op => op.endsWith("permissions.list") ? permissions : [], events);
        const store = createInteractionController(client, "s", false, () => true, s => { snapshot = s; }, () => {});
        await store.refresh(); permissions = [permission(), permission("new")];
        await store.events(); await flush();
        expect(request.mock.calls.filter(([op]) => op.endsWith("permissions.list")).length).toBeGreaterThan(1);
        // The reply removes the already-known request even without a session field. A stale list
        // returning that permission cannot resurrect it; the newly asked permission appears now.
        expect(snapshot.permissions.map(p => p.id)).toEqual(["new"]);
        const scopeReply = { type: "permission.replied", subject: { kind: "session", id: "s" }, data: { requestID: "p" } };
        expect(eventForSession(scopeReply, "s")).toBe(true);
        expect(eventForSession(events[2], "s")).toBe(false);
        store.dispose();
    });
    it("does not send empty or malformed question answers", async () => {
        const { client, request } = mockClient(op => op.endsWith("questions.list") ? [question()] : []);
        const store = createInteractionController(client, "s", false, () => true, () => {}, () => {});
        await store.refresh(); expect(await store.question("q", [{ selected: [], custom: "   " }])).toBe(false);
        expect(request.mock.calls.some(([op]) => op.endsWith("questions.reply"))).toBe(false); store.dispose();
    });
    it("submits normalized complete question answers through the scoped SDK operation", async () => {
        const requestQuestion = question([{ question: "Why?" }, { question: "Pick", multiple: true, options: [{ label: "A" }] }]);
        let snapshot = emptyInteractions();
        const { client, request } = mockClient(op => op.endsWith("questions.list") ? [requestQuestion] : op.endsWith("questions.reply") ? true : []);
        const store = createInteractionController(client, "s", false, () => true, s => { snapshot = s; }, () => {});
        await store.refresh();
        expect(await store.question("q", [{ selected: [], custom: " because " }, { selected: ["A"], custom: " extra " }])).toBe(true);
        await flush();
        expect(request.mock.calls.find(([op]) => op.endsWith("questions.reply"))?.[1]).toMatchObject({ path: { request_id: "q" }, body: { answers: [["because"], ["A", "extra"]] } });
        expect(snapshot.questions).toHaveLength(0); store.dispose();
    });
    it("aborts request signals on unmount and ignores a late server response", async () => {
        const pending = deferred<unknown>(), publish = vi.fn();
        const { client, request } = mockClient(() => pending.promise);
        const store = createInteractionController(client, "s", true, () => true, publish, () => {});
        const fetch = store.refresh(); store.dispose(); pending.resolve([]); await fetch;
        expect(publish).not.toHaveBeenCalled();
        for (const [, input] of request.mock.calls) expect((input as { signal: AbortSignal }).signal.aborted).toBe(true);
    });
    it("coalesces event bursts and never publishes snapshots raced by events", async () => {
        const first = deferred<number>(); const publish = vi.fn(), fetch = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(2);
        const refresh = refreshQueue(fetch, publish, () => true, () => {});
        const pending = refresh(); void refresh(); void refresh(); first.resolve(1); await pending;
        expect(fetch).toHaveBeenCalledTimes(2); expect(publish).toHaveBeenCalledExactlyOnceWith(2);
    });
});

describe("task endpoints and independent lifecycle", () => {
    it("uses actual scoped SDK tasks, children and runtime rather than root Recents", async () => {
        const { client, request } = mockClient(op => op.endsWith("tasks.list") ? [task()] : op.endsWith("children") ? { items: [child] } : runtime());
        const snapshot = await fetchSubagents(client, "s", new AbortController().signal);
        expect(snapshot.rows).toHaveLength(1); expect(snapshot.rows[0].status).toBe("outstanding");
        expect(snapshot.canStop).toBe(true);
        expect(request.mock.calls.map(([op]) => op)).toEqual(["v2.subagents.tasks.list", "v2.sessions.children", "v2.sessions.runtime"]);
        for (const [, input] of request.mock.calls) expect(input).toMatchObject({ path: { session_id: "s" } });
    });
    it("does not mark children completed from an idle/finished root; missing status stays unknown", () => {
        const rootFinished = { ...runtime(), execution: { finished: true } } as SessionRuntimeSnapshot;
        expect(taskRows("s", [], [child], rootFinished)[0]).toMatchObject({ status: "outstanding", stoppable: true });
        expect(taskRows("s", [], [child])[0]).toMatchObject({ status: "unknown", stoppable: false });
        expect(taskRows("s", [task("running")], [], { ...runtime(), branches: [] })[0].status).toBe("running");
        expect(taskRows("s", [task()], [], runtime("failed"))[0]).toMatchObject({ status: "failed", stoppable: false });
        expect(taskRows("s", [task("running")], [], runtime("completed"))[0]).toMatchObject({ status: "running", stoppable: true });
        expect(taskRows("s", [task("error")], [], runtime("completed"))[0].status).toBe("error");
    });
    it("retains child navigation when task plugin/runtime endpoints are unavailable", async () => {
        const { client } = mockClient(op => { if (op.endsWith("children")) return { items: [child] }; throw { status: 404 }; });
        const snapshot = await fetchSubagents(client, "s", new AbortController().signal);
        expect(snapshot.rows[0].sessionId).toBe("child"); expect(snapshot.canStop).toBe(false); expect(snapshot.errors).toHaveLength(2);
    });
    it("guards stale fetch/action results and sends specific task stop IDs", async () => {
        let current = true, snapshot: SubagentSnapshot = emptySubagents(); const result = deferred<unknown>(); const publish = vi.fn((s: SubagentSnapshot) => { snapshot = s; });
        const { client, request } = mockClient(op => op.endsWith("tasks.stop") ? result.promise : op.endsWith("tasks.list") ? [task()] : op.endsWith("children") ? { items: [] } : runtime());
        const store = createSubagentController(client, "s", () => current, publish);
        await store.refresh(); const pending = store.stop("t");
        expect(await store.stop("t")).toBe(false);
        expect(request.mock.calls.find(([op]) => op.endsWith("tasks.stop"))?.[1]).toMatchObject({ path: { session_id: "s" }, body: { taskId: "t" } });
        const count = publish.mock.calls.length; current = false; result.resolve({ stopped: ["child"], clearedPrompts: 2 }); await pending;
        expect(publish).toHaveBeenCalledTimes(count); expect(snapshot.notice).toBe(""); store.dispose();
    });
    it("refreshes on scoped runtime events, ignores foreign ones, and leaves children outstanding on root idle", async () => {
        let snapshot = emptySubagents();
        const { client, request } = mockClient(op => op.endsWith("tasks.list") ? [task()] : op.endsWith("children") ? { items: [] } : runtime(), [
            { type: "session.status", data: { sessionID: "s", status: { type: "idle" } } },
            { type: "session.status", data: { sessionID: "foreign", status: { type: "idle" } } },
        ]);
        const store = createSubagentController(client, "s", () => true, s => { snapshot = s; });
        await store.refresh(); await store.events(); await flush();
        expect(request).toHaveBeenCalledTimes(6);
        expect(snapshot.rows[0].status).toBe("outstanding"); store.dispose();
    });
    it("does not publish task fetches after session changes", async () => {
        const pending = deferred<unknown>(); let current = true;
        const { client } = mockClient(() => pending.promise), publish = vi.fn();
        const store = createSubagentController(client, "s", () => current, publish);
        const fetch = store.refresh(); current = false; pending.resolve([]); await fetch;
        expect(publish).not.toHaveBeenCalled(); store.dispose();
    });
    it("stops all with the native empty-body object and refreshes actual status", async () => {
        let stopped = false, snapshot = emptySubagents();
        const { client, request } = mockClient(op => {
            if (op.endsWith("tasks.stop")) { stopped = true; return { stopped: ["child"], clearedPrompts: 2 }; }
            return op.endsWith("tasks.list") ? [task(stopped ? "completed" : "running")] : op.endsWith("children") ? { items: [] } : runtime(stopped ? "completed" : "outstanding");
        });
        const store = createSubagentController(client, "s", () => true, s => { snapshot = s; });
        await store.refresh(); await store.stop(); await flush();
        expect(request.mock.calls.find(([op]) => op.endsWith("tasks.stop"))?.[1]).toMatchObject({ body: {} });
        expect(snapshot.notice).toContain("cleared 2"); expect(snapshot.rows[0].status).toBe("completed"); store.dispose();
    });
});

describe("readable safe transcript and precise usage", () => {
    it("renders a compact tool header and exposes errors without mounting collapsed output", () => {
        const part = { id: "p", sessionId: "s", messageId: "m", type: "tool", tool: "bash", callId: "call", state: {
            status: "completed", title: "Run tests", input: { command: "npm test" }, output: '<script>alert("x")</script>', metadata: { privateUnusedField: "not presented" }, time: { start: 1000, end: 3500 },
        } } as Part;
        const html = renderToStaticMarkup(<PartView part={part} />);
        expect(html).toContain("Bash"); expect(html).not.toContain("Tool details"); expect(html).not.toContain("<h4>Input</h4>"); expect(html).toContain("npm test"); expect(html).not.toContain("Output preview"); expect(html).toContain("2.5 s"); expect(html).toContain('aria-expanded="false"');
        expect(html).not.toContain("<script>"); expect(html).not.toContain("privateUnusedField");
        expect(renderToStaticMarkup(<PartView part={{ ...part, state: { status: "error", input: {}, error: "Permission denied", time: { start: 0, end: 4 } } } as Part} />)).toContain("Permission denied");
    });
    it("shows reasoning summary and safe Markdown without an ordinary user author heading", () => {
        const reasoning = { id: "p", sessionId: "s", messageId: "m", type: "reasoning", text: "## Checking sources\n\n<script>bad()</script>", time: { start: 0, end: 1200 } } as Part;
        const m = { ...message("m", "s", "Remote Alice"), parts: [reasoning] };
        const html = renderToStaticMarkup(<Timeline messages={[m]} liveParts={new Map([[m.info.id, new Set(['p'])]])} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html).not.toContain("message-label"); expect(messageAuthor(m)).toBe("Remote Alice"); expect(html).toContain("Checking sources"); expect(html).not.toContain("1.2 s"); expect(html).toContain('neo-thinking-part'); expect(html).not.toContain("<script>");
        expect(messageAuthor(message("m"))).toBe("User"); expect(duration({ start: 9, end: 4 })).toBe("");
    });
    it("anchors prepends, follows only at bottom, and resets on session identity", () => {
        const before = { session: "s", ids: ["m2", "m3"], height: 1000, top: 200 };
        expect(scrollPlan(before, "s", ["m1", "m2", "m3"], 1500, true)).toEqual({ mode: "anchor", top: 700 });
        expect(scrollPlan(before, "s", ["m2", "m3", "m4"], 1200, false)).toEqual({ mode: "stay" });
        expect(scrollPlan(before, "s", ["m2", "m3", "m4"], 1200, true)).toEqual({ mode: "follow" });
        expect(scrollPlan(before, "other", ["m5"], 100, false)).toEqual({ mode: "reset" });
    });
    it("keeps cumulative totals separate from native sidebar context", () => {
        const step = { id: "p", sessionId: "s", messageId: "m", type: "step-finish", cost: 0.1, reason: "stop", tokens: { input: 100, output: 20, reasoning: 8, cache: { read: 40, write: 10 } } } as StepFinishPart;
        expect(summarizeUsage([step, step])).toEqual({ cost: 0.1, total: 170, input: 100, output: 20, reasoning: 8, cacheRead: 40, cacheWrite: 10 });
        expect(summarizeUsage([{ ...step, tokens: { ...step.tokens, total: 500 } }]).total).toBe(500);
        const app = { id: "s", client: {} as NeoismClient, active: undefined, prefs: { directory: "" }, usage: [step] } as unknown as ReturnType<typeof useAppController>;
        const html = renderToStaticMarkup(<ChatDetails app={app} />);
        expect(html).toContain("178"); expect(html).not.toMatch(/Cache read|Reasoning|not lifetime usage/);
    });
});
