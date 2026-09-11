import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { ComponentProps } from "react";
import type { StepFinishPart } from "@neoism/sdk";
import { ChatDetails, SidebarSubagents } from "../ChatDetails";
import { emptySubagents } from "../subagentController";
import { activeSidebarTasks, contextCaption, contextFraction, latestSidebarUsage, rawContextLimit, sidebarTokenCount, type SidebarCatalog } from "../sidebarUsage";
import type { TaskRow } from "../chatSupport";

const step = (input: number, cost = 0): StepFinishPart => ({ id: "step", messageId: "message", sessionId: "private-session-id", type: "step-finish", reason: "stop", cost,
    tokens: { input, output: 20, reasoning: 8, cache: { read: 40, write: 10 }, total: 999999 } });
const catalog: SidebarCatalog = { all: [{ id: "provider", name: "Provider", env: [], options: {}, source: "builtin", models: {
    "model/variant": { id: "model/variant", providerId: "provider", name: "Model", api: {}, releaseDate: "", status: "active", limit: { context: 128000, input: 32000, output: 8000 } },
} }], connected: [], default: {} };

describe("native sidebar usage", () => {
    it("uses the latest nonzero step, includes reasoning/cache and ignores provider total", () => {
        const empty = { ...step(0), tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 }, total: 55 } };
        expect(latestSidebarUsage([step(9000, 2), step(100, .1), empty])).toEqual({ context: 178, input: 100, output: 20, reasoning: 8, cacheRead: 40, cacheWrite: 10, cost: 0 });
        expect(latestSidebarUsage([step(9000, 2), step(100, .1)]).cost).toBe(.1);
        expect(latestSidebarUsage([]).context).toBeUndefined();
        expect(latestSidebarUsage([empty]).context).toBeUndefined();
    });
    it("uses raw matching provider/model capacity, never input/output limits", () => {
        expect(rawContextLimit(catalog, "provider/model/variant")).toBe(128000);
        expect(rawContextLimit(catalog, "other/model/variant")).toBeUndefined();
        expect(rawContextLimit(catalog, "provider/missing")).toBeUndefined();
        expect(rawContextLimit(undefined, "provider/model/variant")).toBeUndefined();
        const noLimit = structuredClone(catalog);
        noLimit.all[0].models["model/variant"].limit = { input: 32000 };
        expect(rawContextLimit(noLimit, "provider/model/variant")).toBeUndefined();
    });
    it("clamps known capacity, never fabricates a percentage for unknown values", () => {
        expect(contextFraction(32100, 128000)).toBe(32100 / 128000);
        expect(contextFraction(200000, 128000)).toBe(1);
        expect(contextFraction(-1, 128000)).toBe(0);
        for (const limit of [undefined, 0, -1, Infinity, NaN]) expect(contextFraction(100, limit)).toBeUndefined();
        expect(contextFraction(undefined, 128000)).toBeUndefined();
        expect(contextCaption(32100, 128000)).toBe("32.1k / 128.0k tokens");
        expect(sidebarTokenCount(9999)).toBe("9,999");
        expect(sidebarTokenCount(10000)).toBe("10.0k");
        expect(contextCaption(undefined, undefined)).toBe("— / — tokens");
    });
});

function app(): ComponentProps<typeof ChatDetails>["app"] {
    return { client: {} as ComponentProps<typeof ChatDetails>["app"]["client"], id: "private-session-id", active: undefined,
        prefs: { directory: "/workspace" } as ComponentProps<typeof ChatDetails>["app"]["prefs"], model: "provider/model/variant", agent: "build", thinking: "high",
        usage: [step(9000, 5), step(32022, .1)], providerCatalog: catalog, openSession: async () => {} };
}
const row = (status: string): TaskRow => ({ id: "private-task-id", sessionId: "private-child-id", title: "private-child-id", agent: "explore", status, nested: false, stoppable: true });
const children = (rows: TaskRow[], loading = false) => renderToStaticMarkup(<SidebarSubagents parentId="parent" data={{ ...emptySubagents(), rows, loading, errors: ["private-error-id"], canStop: true }} open={() => {}} />);

describe("native sidebar rendering", () => {
    it("renders only directory and context meter without duplicate session settings or usage tables", () => {
        const html = renderToStaticMarkup(<ChatDetails app={app()} />);
        for (const label of ["Directory", "Usage", "32.1k / 128.0k tokens"]) expect(html).toContain(label);
        for (const absent of ["private-session-id", "Loaded message steps", "Total tokens", "Session", "Agent", "Reasoning", "Cache read", "Cache write", "Last turn price", "$5.1000", "Subagents", "Back"]) expect(html).not.toContain(absent);
        expect(html).toContain('role="meter"');
    });
    it("shows neither a fabricated percentage nor a price for unreported usage", () => {
        const html = renderToStaticMarkup(<ChatDetails app={{ ...app(), usage: [], providerCatalog: undefined }} />);
        expect(html).toContain("— / — tokens");
        for (const absent of ["aria-valuenow", "sidebar-context-fill", "Last turn price"]) expect(html).not.toContain(absent);
    });
    it("hides the entire empty/completed/error/loading child section", () => {
        expect(children([], true)).toBe("");
        expect(children([])).toBe("");
        for (const status of ["completed", "failed", "idle", "unknown", "stopped"]) expect(children([row(status)])).toBe("");
        expect(activeSidebarTasks([{ ...row("running"), sessionId: "parent" }], "parent")).toEqual([]);
    });
    it("renders active children as navigation entries without status or stop controls", () => {
        for (const status of ["running", "pending", "queued", "outstanding", "busy", "retry"]) {
            const html = children([row(status)]);
            expect(html).toContain("Subagents");
            expect(html).toContain("Open subagent");
            expect(html).not.toMatch(/Stop task|Stop all subagents|Refresh|outstanding|explore/);
            expect(html).not.toContain("private-");
        }
        expect(children([{...row("running"),title:"Find definitions (@general subagent)"}])).not.toContain("@general");
        expect(children([{ ...row("running"), title: "Find definitions" }, { ...row("completed"), title: "Old completed task" }])).not.toContain("Old completed task");
    });
});
