import { describe, expect, it } from "vitest";
import type { Event, MessageWithParts, SessionRuntimeSnapshot } from "@neoism/sdk";
import { emptyChat, reconcileRecent, reduceEvent, runtimeIsOlder } from "./state";

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
        const runtime = (executionId: string, revision: number, familyRevision: number) => ({
            rootSessionId: "s", branches: [], familyRevision,
            execution: { executionId, revision },
        }) as unknown as SessionRuntimeSnapshot;
        expect(runtimeIsOlder(runtime("run-1", 3, 1), runtime("run-1", 2, 1))).toBe(true);
        expect(runtimeIsOlder(runtime("run-1", 3, 2), runtime("run-1", 3, 1))).toBe(true);
        expect(runtimeIsOlder(runtime("run-1", 3, 2), runtime("run-2", 0, 0))).toBe(false);
        expect(runtimeIsOlder(runtime("run-2", 0, 0), runtime("run-1", 100, 100))).toBe(true);
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
