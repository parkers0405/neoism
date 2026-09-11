import { reconcileRecent, orderMessages, messageCreated } from "./state";
import { describe, it, expect } from "vitest";
import type { Event, MessageWithParts } from "@neoism/sdk";
import {
    emptyChat,
    applyRuntime,
    runtimeWorking,
    historyCursor,
    mergePage,
    nextCursor,
    reconcile,
    reduceEvent,
} from "./state";
const event = (type: string, data: unknown, id = "e1") =>
    ({
        type,
        data,
        id,
        sequence: 1,
        timestamp: 1,
        schemaVersion: "2",
        source: "test",
    }) as Event;
const message = (id: string, text = "hello"): MessageWithParts => ({
    info: { id, role: "assistant", sessionId: "s", time: { created: id.charCodeAt(0) } },
    parts: [{ id: "p", messageId: id, sessionId: "s", type: "text", text }],
});
const timed = (id: string, role: "user" | "assistant", created?: number, parentId?: string): MessageWithParts => ({
    info: { id, sessionId: "s", role, time: created === undefined ? {} : { created }, ...(parentId ? { parentId } : {}) },
    parts: [],
});
const ids = (messages: MessageWithParts[]) => messages.map((m) => m.info.id);

describe("opaque-ID causal ordering", () => {
    const user = timed("ffe2-uuid-user", "user", 100);
    const step = timed("aaa-response", "assistant", 110, user.info.id);
    const finish = timed("000-finish", "assistant", 300, user.info.id);
    const nextUser = timed("001-next-user", "user", 200);
    const nextReply = timed("zzz-next-reply", "assistant", 210, nextUser.info.id);
    const rows = [user, step, nextUser, nextReply, finish];
    it("preserves chronological interleaving across late completion and shuffled history", () => {
        // All permutations, not a lucky ID/time-aligned fixture.
        const permutations = <T,>(xs: T[]): T[][] => xs.length ? xs.flatMap((x, i) =>
            permutations(xs.filter((_, j) => j !== i)).map((rest) => [x, ...rest])) : [[]];
        for (const permutation of permutations(rows)) {
            expect(ids(orderMessages(permutation))).toEqual(ids(rows));
        }
    });
    it("keeps a steering user between two steps sharing the original parent", () => {
        const a = timed("user-A", "user", 100);
        const a1 = timed("step-1", "assistant", 110, a.info.id);
        const steer = timed("steering-UUID", "user", 120);
        const a2 = timed("step-2", "assistant", 130, a.info.id);
        const expected = [a, a1, steer, a2];
        expect(ids(orderMessages([a2, steer, a1, a]))).toEqual(ids(expected));
        let state = reconcile(emptyChat, [a, a1, steer]);
        state = reduceEvent(state, event("message.updated", {sessionID: "s", info: a2.info}), "s");
        expect(ids(state.messages)).toEqual(ids(expected));
        expect(ids(reconcileRecent(state, [...expected].reverse()).messages)).toEqual(ids(expected));
    });
    it("uses parent constraints only where chronology would put a child first", () => {
        const skewed = timed("skewed", "assistant", 90, user.info.id);
        expect(ids(orderMessages([nextUser, finish, skewed, user]))).toEqual(
            [user.info.id, skewed.info.id, nextUser.info.id, finish.info.id]);
        const unknown = timed("unknown-parent", "user");
        const child = timed("child", "assistant", 1, unknown.info.id);
        expect(ids(orderMessages([child, user, unknown]))).toEqual([user.info.id, unknown.info.id, child.info.id]);
    });
    it("does not role-sort a second user or regroup equal-time assistant steps", () => {
        const a = timed("z-user", "user", 100);
        const first = timed("first", "assistant", 100, a.info.id);
        const steer = timed("a-steering", "user", 100);
        const last = timed("last", "assistant", 100, a.info.id);
        const canonical = [a, first, steer, last];
        expect(ids(orderMessages(canonical))).toEqual(ids(canonical));
        expect(ids(reconcile(reconcile(emptyChat, canonical), [...canonical].reverse()).messages)).toEqual(ids(canonical));
        // Repair only the violated edge; do not promote a user whose child is
        // already after it, or collect siblings around the repaired parent.
        expect(ids(orderMessages([first, a, steer, last]))).toEqual(ids(canonical));
    });
    it("keeps equal times stable but always emits a user before its responses", () => {
        const u = timed("z", "user", 100);
        const a = timed("b", "assistant", 100, "z");
        const b = timed("a", "assistant", 100, "z");
        const first = reconcile(emptyChat, [a, b, u]);
        expect(ids(first.messages)).toEqual(["z", "b", "a"]);
        expect(ids(reconcile(first, [b, u, a]).messages)).toEqual(["z", "b", "a"]);
    });
    it("corrects a part-before-metadata placeholder after a late UUID user update", () => {
        let state = reduceEvent(emptyChat, event("message.part.delta", {
            sessionID: "s", messageID: step.info.id, partID: "p", partType: "text", field: "text", delta: "fixture",
        }), "s");
        expect(messageCreated(state.messages[0])).toBeUndefined();
        state = reduceEvent(state, event("message.updated", {sessionID: "s", info: step.info}, "meta-a"), "s");
        state = reduceEvent(state, event("message.updated", {sessionID: "s", info: user.info}, "meta-u"), "s");
        expect(ids(state.messages)).toEqual([user.info.id, step.info.id]);
        expect(state.messages[1].parts[0].text).toBe("fixture");
        expect(ids(reconcileRecent(state, [step, user], true).messages)).toEqual(ids(state.messages));
    });
    it("keeps synthetic runtime users as causal roots without manufacturing time", () => {
        const runtime = timed("runtime:task:uuid", "user");
        const reply = timed("a", "assistant", 1, runtime.info.id);
        expect(ids(orderMessages([reply, user, runtime]))).toEqual([user.info.id, runtime.info.id, "a"]);
        expect(runtime.info.time).toEqual({});
        for (const invalid of [NaN, Infinity, -1, "100", null]) {
            expect(messageCreated(timed("bad", "user", invalid as number))).toBeUndefined();
        }
    });
    it("preserves server cursor order independently of display and prepends", () => {
        const page = [step, nextUser, user];
        expect(historyCursor(page, 3)).toBe(user.info.id);
        const state = reconcile(reconcile(emptyChat, [nextUser, nextReply]), [finish, step, user]);
        expect(ids(state.messages)).toEqual(ids(rows));
        expect(ids(reconcile(state, rows).messages)).toEqual(ids(rows));
        expect(ids(page)).toEqual([step.info.id, nextUser.info.id, user.info.id]);
    });
    it("does not resurrect tombstones through quiet snapshots, prepends, or late events", () => {
        let state = reconcile(emptyChat, rows);
        state = reduceEvent(state, event("message.removed", {sessionID: "s", messageID: step.info.id}), "s");
        state = reconcileRecent(state, rows);
        state = reconcile(state, rows);
        state = reduceEvent(state, event("message.updated", {sessionID: "s", info: step.info}, "late"), "s");
        expect(ids(state.messages)).not.toContain(step.info.id);
    });
    it("preserves older/unknown pages and uses snapshot membership, never opaque-ID cutoffs", () => {
        const old = timed("zz-old", "user", 1);
        const unknown = timed("00-unknown", "assistant");
        let state = reconcileRecent(emptyChat, [nextUser, nextReply]);
        state = reconcile(state, [old, unknown, user]);
        state = reconcileRecent(state, [nextUser]);
        expect(ids(state.messages)).toEqual([old.info.id, user.info.id, nextUser.info.id, unknown.info.id]);
        // Sliding the recent window forward must not delete its paged-out member.
        const newer = timed("00-new", "user", 400);
        expect(ids(reconcileRecent(state, [newer]).messages)).toContain(nextUser.info.id);
    });
    it("replays the running server's nonchronological ID metadata without transcript text", () => {
        const before = timed("msg_08bf93bbd001Lzpf5ei8ld4L5c", "assistant", 1789054761917, "unloaded-original-user");
        const steering = timed("msg_01a08bf93bc40000000000003f", "user", 1789054783302);
        const after = timed("msg_08bf98fba0016dVVtnc1w2YdQj", "assistant", 1789054783418, "unloaded-original-user");
        expect(ids(reconcile(emptyChat, [after, steering, before]).messages)).toEqual(ids([before, steering, after]));
    });
    it("handles burst SSE, missing time, removals and an older parent arriving in a page", () => {
        let state = emptyChat;
        for (const [i, row] of [nextReply, finish, nextUser, step].entries()) {
            state = reduceEvent(state, event("message.updated", { sessionID: "s", info: row.info }, `burst-${i}`), "s");
        }
        state = reconcile(state, [user]);
        expect(ids(state.messages)).toEqual(ids(rows));
        state = reduceEvent(state, event("message.updated", {sessionID: "s", info: {...step.info, time: {}}}, "partial"), "s");
        expect(messageCreated(state.messages[1])).toBe(110);
        state = reduceEvent(state, event("message.removed", {sessionID: "s", messageID: finish.info.id}, "remove"), "s");
        expect(ids(state.messages)).toEqual(ids([user, step, nextUser, nextReply]));
        state = reduceEvent(state, event("message.part.delta", {sessionID: "s", messageID: finish.info.id,
            partID: "late-part", partType: "text", field: "text", delta: "fixture"}, "late-delta"), "s");
        expect(ids(state.messages)).not.toContain(finish.info.id);
    });
    it("terminates on malformed parent cycles and preserves every message", () => {
        const a = timed("a", "assistant", 1, "b");
        const b = timed("b", "assistant", 2, "a");
        expect(ids(orderMessages([b, a]))).toEqual(["a", "b"]);
    });
    it("keeps equal-time omissions and unknown snapshot windows conservatively", () => {
        const a = timed("a", "user", 1), b = timed("b", "user", 1);
        const state = reconcileRecent(emptyChat, [a, b]);
        expect(ids(reconcileRecent(state, [b]).messages)).toEqual(["a", "b"]);
        expect(ids(reconcileRecent(state, [timed("c", "user")]).messages)).toEqual(["a", "b", "c"]);
    });
});

describe("session history", () => {
    it("pages older V2 history using the final descending message ID", () => {
        const rows = [message("c"), message("b")];
        expect(historyCursor(rows, 2)).toBe("b");
        expect(historyCursor(rows, 3)).toBeUndefined();
        expect(historyCursor(rows, 2, "b")).toBeUndefined();
        expect(historyCursor([], 2)).toBeUndefined();
        expect(historyCursor(rows, 2, undefined, "server-cursor")).toBe("server-cursor");
    });
    it("merges overlapping pages without duplicates", () =>
        expect(
            mergePage(
                [{ id: "a", v: 1 }],
                [
                    { id: "a", v: 2 },
                    { id: "b", v: 3 },
                ],
                (x) => x.id,
            ),
        ).toEqual([
            { id: "a", v: 2 },
            { id: "b", v: 3 },
        ]));
    it("stops repeated and absent cursors", () => {
        expect(nextCursor("x", "x")).toBeUndefined();
        expect(nextCursor("x", undefined)).toBeUndefined();
        expect(nextCursor("x", "y")).toBe("y");
    });
    it("sorts older pages chronologically", () =>
        expect(
            reconcile({ ...emptyChat, messages: [message("b")] }, [
                message("a"),
            ]).messages.map((m) => m.info.id),
        ).toEqual(["a", "b"]));
});
describe("SDK SSE reducer", () => {
    it("keeps outstanding child work active after the root becomes idle", () => {
        const state = applyRuntime(emptyChat, {
            revision: 2, rootSessionId: "s",
            branches: [{ sessionId: "child", parentSessionId: "s", status: "outstanding" }],
        });
        const idle = reduceEvent(state, event("session.status", {sessionID:"s",status:{type:"idle"}}), "s");
        expect(idle.busy).toBe(false);
        expect(runtimeWorking(idle.runtime)).toBe(true);
        expect(runtimeWorking(applyRuntime(idle, {revision:3,rootSessionId:"s",branches:[]}).runtime)).toBe(false);
        expect(applyRuntime(state, {revision:1,rootSessionId:"s",branches:[]})).toBe(state);
    });
    it("uses sessionID event casing and ignores other sessions", () => {
        const e = event("message.part.delta", {
            sessionID: "s",
            messageID: "m",
            partID: "p",
            partType: "text",
            field: "text",
            delta: "hi",
        });
        expect(reduceEvent(emptyChat, e, "other")).toBe(emptyChat);
        const s = reduceEvent(emptyChat, e, "s");
        expect(s.messages[0].parts[0].text).toBe("hi");
        expect(reduceEvent(s, e, "s")).toBe(s);
    });
    it("full part update replaces deltas rather than appending", () => {
        let s = reduceEvent(
            emptyChat,
            event("message.part.delta", {
                sessionID: "s",
                messageID: "m",
                partID: "p",
                partType: "text",
                field: "text",
                delta: "he",
            }),
            "s",
        );
        s = reduceEvent(
            s,
            event(
                "message.part.updated",
                { sessionID: "s", part: message("m").parts[0] },
                "e2",
            ),
            "s",
        );
        expect(s.messages[0].parts[0].text).toBe("hello");
    });
    it("message metadata retains parts and corrects roles", () => {
        const s = reconcile(emptyChat, [message("m")]);
        const next = reduceEvent(
            s,
            event("message.updated", {
                sessionID: "s",
                info: { id: "m", sessionId: "s", role: "user", time: {} },
            }),
            "s",
        );
        expect(next.messages[0].parts).toEqual(s.messages[0].parts);
        expect(next.messages[0].info.role).toBe("user");
    });
    it("removes messages and tracks terminal status", () => {
        const s = reconcile(emptyChat, [message("m")]);
        expect(
            reduceEvent(
                s,
                event("message.removed", { sessionID: "s", messageID: "m" }),
                "s",
            ).messages,
        ).toEqual([]);
        expect(
            reduceEvent(
                s,
                event("session.status", {
                    sessionID: "s",
                    status: { type: "busy" },
                }),
                "s",
            ).busy,
        ).toBe(true);
        expect(
            reduceEvent(
                { ...s, busy: true },
                event("session.status", {
                    sessionID: "s",
                    status: { type: "idle" },
                }),
                "s",
            ).busy,
        ).toBe(false);
    });
});
describe("reconnect reconciliation", () => {
    it("loads earlier history while retaining a concurrently streamed message", () => {
        const live = { ...emptyChat, messages: [message("m", "live")] };
        const result = reconcileRecent(live, [message("m", "snapshot"), message("a")], true);
        expect(result.messages.map((m) => m.info.id)).toEqual(["a", "m"]);
        expect(result.messages[1].parts[0].text).toBe("live");
    });
    it("does not resurrect a message deleted during a history request", () => {
        const live = reduceEvent(
            {...emptyChat, messages: [message("a"), message("m")]},
            event("message.removed", {sessionID:"s",messageID:"a"}), "s",
        );
        expect(reconcileRecent(live, [message("m"), message("a")], true).messages.map((m) => m.info.id)).toEqual(["m"]);
    });
    it("does not double append racing streamed deltas", () => {
        const live = { ...emptyChat, messages: [message("m", "hello")] };
        expect(reconcileRecent(live, [message("m", "hello")], true)).toBe(live);
    });
    it("removes reverted recent messages and retains loaded older pages", () => {
        const live = {
            ...emptyChat,
            recentMessageIds: ["b", "c"],
            messages: [message("a"), message("b"), message("c")],
        };
        expect(
            reconcileRecent(live, [message("b")]).messages.map(
                (m) => m.info.id,
            ),
        ).toEqual(["a", "b"]);
    });
    it("reconciles an empty server history", () =>
        expect(
            reconcileRecent({ ...emptyChat, messages: [message("m")] }, [])
                .messages,
        ).toEqual([]));
});
