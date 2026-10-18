import { describe, expect, it } from "vitest";
import type { Event, MessageWithParts, SessionRuntimeSnapshot } from "@neoism/sdk";
import { applyRuntime, emptyChat, reconcileRecent, reduceEvent, runtimeIsOlder } from "./state";

const row = (text: string): MessageWithParts => ({
    info: { id: "m", sessionId: "s", role: "assistant", time: { created: 1 } },
    parts: [{ id: "p", messageId: "m", sessionId: "s", type: "text", text }],
});
const event = (id: string, type: string, data: unknown) => ({
    id, type, data, sequence: 0, timestamp: 1, source: "test", schemaVersion: "1.0.0",
}) as Event;
const snapshot = (id: string, text: string) => event(id, "message.part.updated", { sessionID: "s", part: row(text).parts[0] });
const delta = (id: string, text: string) => event(id, "message.part.delta", {
    sessionID: "s", messageID: "m", partID: "p", partType: "text", field: "text", delta: text,
});

describe("live attachment baselines", () => {
    it("orders current familyRevision snapshots by execution before the branch revision", () => {
        const runtime = (executionId: string, revision: number, familyRevision: number): SessionRuntimeSnapshot => ({
            rootSessionId: "s", branches: [{ sessionId: "child", parentSessionId: "s", status: familyRevision > 1 ? "completed" : "outstanding" }], familyRevision,
            execution: { executionId, revision, rootSessionId: "s", rootMessageId: "u", finished: false,
                completedMs: 0, activeSegments: {}, sessionActivities: { s: { completedMs: 0, activeSegments: {} } } },
        });
        expect(runtimeIsOlder(runtime("run-1", 3, 1), runtime("run-1", 2, 1))).toBe(true);
        expect(runtimeIsOlder(runtime("run-1", 3, 2), runtime("run-1", 3, 1))).toBe(true);
        expect(runtimeIsOlder(runtime("run-1", 3, 2), runtime("run-2", 0, 0))).toBe(false);
        expect(runtimeIsOlder(runtime("run-2", 0, 0), runtime("run-1", 100, 100))).toBe(true);
        // A timer update wins independently of a lower family revision; a family
        // update wins when the execution revision is unchanged.
        expect(runtimeIsOlder(runtime("run-1", 2, 10), runtime("run-1", 3, 1))).toBe(false);
        expect(runtimeIsOlder(runtime("run-1", 3, 1), runtime("run-1", 3, 2))).toBe(false);
        const current = { ...emptyChat, runtime: runtime("run-1", 3, 2) };
        expect(applyRuntime(current, runtime("run-1", 3, 1))).toBe(current);
        expect(current.runtime.branches[0].status).toBe("completed");
    });
    it("orders branch-only snapshots by familyRevision without an execution clock", () => {
        const current: SessionRuntimeSnapshot = { rootSessionId: "s", familyRevision: 2,
            branches: [{ sessionId: "child", parentSessionId: "s", status: "completed" }] };
        const older: SessionRuntimeSnapshot = { rootSessionId: "s", familyRevision: 1,
            branches: [{ sessionId: "child", parentSessionId: "s", status: "outstanding" }] };
        expect(runtimeIsOlder(current, older)).toBe(true);
        expect(runtimeIsOlder(older, current)).toBe(false);
        expect(runtimeIsOlder(current, { ...older, rootSessionId: "another-root" })).toBe(false);
    });
    it("replaces a cached suffix, then applies subsequent tokens exactly once", () => {
        let state = reconcileRecent(emptyChat, [row("suffix")]);
        state = reduceEvent(state, snapshot("attach", "prefix suffix"), "s");
        state = reduceEvent(state, delta("next", " next"), "s");
        state = reduceEvent(state, delta("next", " next"), "s");
        expect(state.messages[0].parts[0].text).toBe("prefix suffix next");
        state = reduceEvent(state, snapshot("reconnect", "prefix suffix next missed"), "s");
        state = reduceEvent(state, delta("last", " last"), "s");
        expect(state.messages[0].parts[0].text).toBe("prefix suffix next missed last");
    });
    it("does not erase a live baseline during a quiet or racing stale history fetch", () => {
        let state = reconcileRecent(emptyChat, [row("")]);
        state = reduceEvent(state, snapshot("attach", "current prefix"), "s");
        for (const raced of [false, true]) {
            state = reconcileRecent(state, [row("")], raced);
            expect(state.messages[0].parts[0].text).toBe("current prefix");
        }
        state = reduceEvent(state, delta("next", " next"), "s");
        expect(state.messages[0].parts[0].text).toBe("current prefix next");
    });
    it("honors an empty retry baseline and authoritative completion", () => {
        let state = reconcileRecent(emptyChat, [row("abandoned")]);
        state = reduceEvent(state, snapshot("retry", ""), "s");
        state = reduceEvent(state, delta("next", "fresh"), "s");
        state = reduceEvent(state, snapshot("final", "fresh"), "s");
        expect(state.messages[0].parts[0].text).toBe("fresh");
        const final = row("full final answer");
        final.info.time.completed = 2;
        state = reconcileRecent(state, [final]);
        expect(state.messages[0].parts[0].text).toBe("full final answer");
    });
});
