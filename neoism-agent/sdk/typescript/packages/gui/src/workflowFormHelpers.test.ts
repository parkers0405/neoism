import { describe, expect, it, vi } from "vitest";
import type { NeoismClient, WorkflowDefinition, WorkflowSchedule } from "@neoism/sdk";
import { frequencies, loadWorkflowCatalogs, onceMode, patchWorkflow, permissionErrors, retryDefaults, scopeGuard, timezones, workflowErrors, workflowFrequency } from "./workflowFormHelpers";

const schedule: WorkflowSchedule = { frequency: "daily", interval: 1, timezone: "UTC", time: "09:30" };
const definition: WorkflowDefinition = { id: "report", name: "Report", prompt: "Write a report", active: false, schedule };
describe("workflow form schedule normalization and validation", () => {
    it.each(frequencies)("removes incompatible fields when changing to %s", frequency => {
        const dirty = { ...schedule, minute: 12, weekdays: ["friday"], monthDay: 23, date: "2028-05-02", at: "2028-05-02T12:00:00Z" };
        const result = workflowFrequency(dirty, frequency);
        expect(result.timezone).toBe("UTC");
        expect(workflowErrors({ ...definition, schedule: result })).toEqual([]);
        const keys = { hourly: ["minute"], daily: ["time"], weekly: ["time", "weekdays"], monthly: ["time", "monthDay"], once: ["date", "time"] }[frequency];
        expect(Object.keys(result).sort()).toEqual(["frequency", "interval", "timezone", ...keys].sort());
    });
    it("switches once modes without inventing a date or timestamp", () => {
        expect(onceMode(schedule, true)).toEqual({ frequency: "once", interval: 1, timezone: "UTC", at: "" });
        expect(onceMode(schedule, false)).toEqual({ frequency: "once", interval: 1, timezone: "UTC", date: "", time: "00:00" });
        expect(workflowErrors({ ...definition, schedule: onceMode(schedule, true) }).join()).toContain("Timestamp");
    });
    it.each([
        { frequency: "hourly", minute: 60 },
        { frequency: "weekly", weekdays: [] },
        { frequency: "weekly", weekdays: ["mon", "monday"] },
        { frequency: "weekly", weekdays: ["mondayish"] },
        { frequency: "monthly", monthDay: 32 },
        { frequency: "once", date: "2028-02-30" },
        { frequency: "once", at: "2028-02-01T09:00:00" },
        { frequency: "once", at: "2028-02-30T09:00:00Z" },
        { frequency: "once", at: "2028-02-01T24:00:00Z" },
        { frequency: "once", at: "2028-02-01T09:00:00Z", time: "09:00" },
        { frequency: "daily", interval: 0 },
        { frequency: "daily", time: "25:00" },
        { frequency: "daily", timezone: "Not/AZone" },
    ])("rejects invalid schedule %j", fields => {
        expect(workflowErrors({ ...definition, schedule: { interval: 1, timezone: "UTC", ...fields } }).length).toBeGreaterThan(0);
    });
    it("accepts explicit timestamps, server-local timezone and native time forms", () => {
        for (const time of ["00:00", "23:59:59", "9:30 PM", "09:30:00 AM"]) expect(workflowErrors({ ...definition, schedule: { ...schedule, timezone: "local", time } })).toEqual([]);
        expect(workflowErrors({ ...definition, schedule: { frequency: "once", interval: 1, timezone: "UTC", at: "2028-01-02T03:04:05+05:30" } })).toEqual([]);
    });
    it("lists IANA zones with a host fallback when supportedValuesOf is unavailable", () => {
        expect(timezones()).toContain("UTC");
        const spy = vi.spyOn(Intl, "supportedValuesOf").mockImplementation(() => { throw Error("unsupported"); });
        expect(timezones()).toContain(Intl.DateTimeFormat().resolvedOptions().timeZone);
        spy.mockRestore();
    });
});
describe("permissions match native denyUnknownFields and no ask policy", () => {
    it("preserves arbitrary permission names and exact supported patterns", () => {
        const permissions = { "plugin.custom": { allow: ["git diff *", "path with spaces/*"], deny: ["rm *"], ask: [] }, "*": "deny" };
        expect(permissionErrors(permissions)).toEqual([]);
        expect(patchWorkflow({ ...definition, permissions }, { name: "Changed" }).permissions).toBe(permissions);
    });
    it.each([{ bash: "ask" }, { bash: { default: "ask", allow: ["*"] } }, { bash: { ask: ["git *"], deny: ["*"] } }])("rejects every effective ask, even with overriding deny %j", p => expect(permissionErrors(p).join()).toContain("cannot ask"));
    it.each([{ bash: {} }, { " ": "deny" }, { bash: { allow: [" "] } }, { bash: { extra: "allow" } }, { bash: { allow: "*" } }, { bash: { default: "invalid" } }])("rejects invalid rules %j", p => expect(permissionErrors(p).length).toBeGreaterThan(0));
});
describe("native defaults, model preservation and execution directory", () => {
    it("roundtrips omissions without injecting retry or concurrency defaults", () => {
        expect(retryDefaults).toEqual({ maxAttempts: 1, backoff: "fixed", initialDelayMs: 0, maxDelayMs: 0, retryableErrors: [] });
        expect(patchWorkflow(definition, {})).toEqual(definition);
        expect(workflowErrors(definition)).toEqual([]);
    });
    it("retains unknown model, account and variant during unrelated edits", () => {
        const model = { providerId: "custom", id: "private", connectionId: "old-account", variant: "legacy" };
        expect(patchWorkflow({ ...definition, model }, { prompt: "Changed" }).model).toEqual(model);
    });
    it("omits blank execution directories and rejects unsafe relative ones", () => {
        expect(patchWorkflow({ ...definition, directory: "/execution" }, { directory: "  " })).not.toHaveProperty("directory");
        expect(workflowErrors({ ...definition, directory: "../outside" }).join()).toContain("Relative execution");
        expect(workflowErrors({ ...definition, directory: "~/outside" })).toEqual([]);
    });
    it("validates retries, concurrency and all integer bounds", () => {
        expect(workflowErrors({ ...definition, retry: { initialDelayMs: 1000, maxDelayMs: 0 } })).toEqual([]);
        expect(workflowErrors({ ...definition, retry: { initialDelayMs: 1000, maxDelayMs: 900 } }).join()).toContain("Maximum retry delay");
        for (const maxRunning of [0, -1, 1.2, NaN]) expect(workflowErrors({ ...definition, concurrency: { mode: "allow", maxRunning } }).length).toBeGreaterThan(0);
        expect(workflowErrors({ ...definition, concurrency: { mode: "replace", maxRunning: 2 } }).join()).toContain("maximum running = 1");
        expect(workflowErrors({ ...definition, concurrency: { mode: "allow", maxRunning: 2 } })).toEqual([]);
    });
});
describe("project catalog scope", () => {
    it("uses installation config catalogs by default without any selected directory", async () => {
        const request = vi.fn(async (operation: string) => operation === "v2.providers.configured" ? { providers: [] } : []);
        const client = { operations: { request } } as unknown as NeoismClient;
        await loadWorkflowCatalogs(client);
        expect(request.mock.calls.map(call => call[0])).toEqual(["v2.agents.list", "v2.skills.list", "v2.providers.configured"]);
        for (const operation of ["v2.agents.list", "v2.skills.list", "v2.providers.configured"]) expect(request).toHaveBeenCalledWith(operation, { query: { scope: "installation" } });
    });
    it("uses only the supplied project root and filters hidden/subagents", async () => {
        const agents = vi.fn().mockResolvedValue([{ name: "primary", mode: "primary", hidden: false }, { name: "both", mode: "all", hidden: false }, { name: "hidden", hidden: true }, { name: "child", mode: "subagent" }]);
        const skills = vi.fn().mockResolvedValue([{ name: "project-skill" }]);
        const configured = vi.fn().mockResolvedValue({ providers: [] });
        const client = { catalog: { agents: { list: agents }, skills: { list: skills }, providers: { configured } } } as unknown as NeoismClient;
        const result = await loadWorkflowCatalogs(client, "/storage-project");
        for (const fn of [agents, skills, configured]) expect(fn).toHaveBeenCalledWith("/storage-project");
        expect(result.agents.map(a => a.name)).toEqual(["primary", "both"]);
    });
    it("ignores late success and failure after scope cancellation", async () => {
        const guard = scopeGuard(); const success = vi.fn(); const failure = vi.fn();
        let resolve!: (value: string) => void; let reject!: (error: unknown) => void;
        const pending = guard.run(new Promise<string>(r => { resolve = r; }), success, failure);
        const failed = guard.run(new Promise<string>((_, r) => { reject = r; }), success, failure);
        guard.cancel(); resolve("old directory"); reject(Error("old directory"));
        await Promise.all([pending, failed]); expect(success).not.toHaveBeenCalled(); expect(failure).not.toHaveBeenCalled();
        const current = scopeGuard(); await current.run(Promise.resolve("new directory"), success, failure); expect(success).toHaveBeenCalledWith("new directory");
    });
});
