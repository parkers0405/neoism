import { describe, expect, it } from "vitest";
import type { MessageWithParts, SessionRuntimeSnapshot } from "@neoism/sdk";
import { ACTIVITY, STATUS, activityLabel, dotFrame, elapsedLabel, glyphFrame, phaseSeconds, resolveActivity, runtimeElapsed } from "./nativeActivityMotion";
import { readFileSync } from "node:fs";
const native = readFileSync(new URL("../../../../../../../neoism-frontend/shared/src/panels/agent_pane/view/user_input.rs", import.meta.url), "utf8");
const message = (type: string, completed?: number) => [{ info: { id: "m", sessionId: "root", role: "assistant", time: { completed } }, parts: [{ id: "p", type, ...(type === "tool" ? { state: { status: "running" } } : {}) }] }] as unknown as MessageWithParts[];
const runtime = (overrides: Partial<SessionRuntimeSnapshot> = {}): SessionRuntimeSnapshot => ({ rootSessionId: "root", revision: 1, branches: [], ...overrides });
describe("native activity motion port", () => {
    it("pins exact native constants and face, not sidebar orbit or generic pulse", () => {
        expect(ACTIVITY.scramble).toBe("|/-\\+!?>?<%#=@*~&^$");
        for (const source of ["const SCRAMBLE_TOTAL: f32 = 0.7", "now_seconds * 44.0", "live_phase * 5.6 + ix as f32 * 0.82", "live_phase * 4.0 + ix as f32 * 0.95", "Press Start 2P", "word_motion.sin() * 1.8", "wave_phase.sin() * 2.4", "cursor_x += 7.0", "0.40 + (swell * 0.5 + 0.5) * 0.45"]) expect(native).toContain(source);
        expect(ACTIVITY).toMatchObject({ scrambleSeconds: 0.7, scrambleHz: 44, lineHeight: 26, inset: 3.3 });
        expect(STATUS.thinking[0]).toBe("Pondering"); expect(STATUS.working[0]).toBe("Tinkering");
    });
    it("locks each character at its native threshold, resolving the last at 700ms", () => {
        expect(glyphFrame("C", 0, 7, 0, 0).char).toBe("|");
        expect(glyphFrame("r", 1, 7, 0, 0).char).toBe("!");
        expect(glyphFrame("C", 0, 7, 0, .1).char).toBe("C");
        expect(glyphFrame("g", 6, 7, 0, .699).char).not.toBe("g");
        expect(glyphFrame("g", 6, 7, 0, .7).char).toBe("g");
        expect(glyphFrame("C", 0, 7, 0, 1)).toMatchObject({ x: 1.5, y: -.8 });
        expect(glyphFrame("C", 0, 7, 0, 1).mix).toBeCloseTo(.19);
    });
    it("uses ocean swell, not a stepped dot opacity/pulse", () => {
        const dot = dotFrame(0, 0);
        expect(dot.x).toBeCloseTo(Math.cos(1.2)); expect(dot.y).toBe(-.85);
        expect(dot.alpha).toBe(159 / 255);
        expect(dotFrame(1, 0)).not.toEqual(dot);
    });
    it("reduced motion resolves immediately and eliminates drift, wave and scramble", () => {
        expect(glyphFrame("C", 0, 7, 123, 0, true)).toEqual({ char: "C", x: 0, y: 0, mix: 0 });
        expect(dotFrame(2, 123, true)).toEqual({ x: 0, y: 0, alpha: 1 });
    });
    it("wraps monotonic phase safely and keeps epoch only in timer sampling", () => {
        expect(phaseSeconds(10_000_250)).toBe(.25);
        expect(phaseSeconds(-1)).toBe(0);
        const r = runtime({ execution: { executionId: "e", rootSessionId: "root", rootMessageId: "m", revision: 1, finished: false, completedMs: 500, activeSegments: { a: 1000, b: 1500 } } });
        expect(runtimeElapsed(r, 2000)).toBe(2);
        expect(elapsedLabel(2)).toBe("2.0s"); expect(elapsedLabel(61)).toBe("1m 1s"); expect(elapsedLabel(5460)).toBe("1h 31m");
    });
});
describe("native status selection", () => {
    it("shows live reasoning, tools, or response and hides idle stale reasoning", () => {
        expect(resolveActivity(message("reasoning"), true).status).toBe("thinking");
        expect(resolveActivity(message("tool"), true).status).toBe("working");
        expect(resolveActivity(message("text"), true).status).toBe("generating");
        expect(resolveActivity(message("reasoning"), false).status).toBe("idle");
    });
    it("does not mistake runtime-derived busy for main streaming after main completion", () => {
        const r = runtime({ branches: [{ parentSessionId: "root", sessionId: "child", status: "outstanding" }] });
        expect(resolveActivity(message("text", 1), true, r).status).toBe("waitingSubagents");
        expect(resolveActivity(message("text", 1), true, r, undefined, "child").status).toBe("working");
        expect(resolveActivity([], true, runtime({ runningBackgroundTasks: [{ sessionId: "root", jobId: "j", startedAt: 1 }] })).status).toBe("backgroundTasks");
        r.execution = { executionId: "e", rootSessionId: "root", rootMessageId: "m", revision: 1, finished: false, completedMs: 0, activeSegments: {} };
        expect(resolveActivity(message("reasoning"), true, r).status).toBe("waitingSubagents");
    });
    it("uses provider segments, not unfinished execution or stale reasoning, for background-only activity", () => {
        const r = runtime({ execution: { executionId: "e", rootSessionId: "root", rootMessageId: "u", revision: 1, finished: false, completedMs: 0, activeSegments: {} },
            runningBackgroundTasks: [{ sessionId: "root", jobId: "job", startedAt: 1 }] });
        for (const messages of [[], message("reasoning"), message("text", 1), [{ info: { role: "user" } }] as MessageWithParts[]]) {
            expect(resolveActivity(messages, true, r).status).toBe("backgroundTasks");
        }
        r.execution!.activeSegments = { provider: 1 };
        expect(resolveActivity(message("text"), true, r).status).toBe("generating");
        expect(resolveActivity(message("reasoning"), true, r).status).toBe("thinking");
        expect(resolveActivity(message("reasoning", 1), true, r).status).toBe("generating");
        const ended = message("reasoning");
        Object.assign(ended[0].parts[0], { time: { start: 1, end: 2 } });
        expect(resolveActivity(ended, true, r).status).toBe("generating");
        r.execution!.finished = true;
        r.runningBackgroundTasks = [];
        expect(resolveActivity(message("reasoning"), true, r).status).toBe("idle");
    });
    it("uses the viewed session's provider activity rather than family aggregate segments", () => {
        const r = runtime({ branches: [{ sessionId: "child", parentSessionId: "root", status: "outstanding" }],
            execution: { executionId: "e", rootSessionId: "root", rootMessageId: "u", revision: 1, finished: false, completedMs: 0, activeSegments: { provider: 1 } } });
        Object.assign(r.execution!, { sessionActivities: { root: { activeSegments: {} }, child: { activeSegments: { provider: 1 } } } });
        expect(resolveActivity(message("reasoning"), true, r, undefined, "root").status).toBe("waitingSubagents");
        const child = message("reasoning"); child[0].info.sessionId = "child";
        expect(resolveActivity(child, false, r, undefined, "child").status).toBe("thinking");
        expect(resolveActivity(message("reasoning"), false, r, undefined, "child").status).toBe("generating");
        expect(resolveActivity(child, true, r, undefined, "other").status).toBe("idle");
        expect(resolveActivity([], false, runtime(), { status: "idle", queuedCount: 2 }).status).toBe("idle");
        expect(resolveActivity([], false, runtime(), { status: "idle", backgroundCount: 1 }).status).toBe("backgroundTasks");
    });
    it("accepts authoritative retry/compaction states absent from runtime schema", () => {
        expect(resolveActivity([], true, undefined, { status: "compacting" }).status).toBe("compacting");
        expect(activityLabel({ status: "retrying", retryReason: "HTTP 429 too many requests" })).toBe("Retrying · Rate limited");
        expect(activityLabel({ status: "retrying", retryReason: "   " })).toBe("Retrying");
    });
});
