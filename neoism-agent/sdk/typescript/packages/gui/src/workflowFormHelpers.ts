import type { NeoismClient, WorkflowDefinition, WorkflowSchedule } from "@neoism/sdk";

export const frequencies = ["hourly", "daily", "weekly", "monthly", "once"] as const;
export const weekdays = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
export const retryDefaults = { maxAttempts: 1, backoff: "fixed" as const, initialDelayMs: 0, maxDelayMs: 0, retryableErrors: [] as string[] };
export function workflowFrequency(s: WorkflowSchedule, frequency: string): WorkflowSchedule {
    const base = { frequency, interval: frequency === "once" ? 1 : s.interval, timezone: s.timezone };
    switch (frequency) {
        case "hourly": return { ...base, minute: s.minute ?? 0 };
        case "daily": return { ...base, time: s.time ?? "00:00" };
        case "weekly": return { ...base, time: s.time ?? "00:00", weekdays: s.weekdays?.length ? s.weekdays : ["monday"] };
        case "monthly": return { ...base, time: s.time ?? "00:00", monthDay: s.monthDay ?? 1 };
        case "once": return s.frequency === "once" && s.at !== undefined ? { ...base, at: s.at } : { ...base, date: s.date ?? "", time: s.time ?? "00:00" };
        default: return base;
    }
}
export function onceMode(s: WorkflowSchedule, timestamp: boolean): WorkflowSchedule {
    const base = { frequency: "once", interval: 1, timezone: s.timezone };
    return timestamp ? { ...base, at: "" } : { ...base, date: "", time: "00:00" };
}
/** Optional blank execution directory is omitted, never filled from the catalog root. */
export function patchWorkflow(value: WorkflowDefinition, patch: Partial<WorkflowDefinition>): WorkflowDefinition {
    const next = { ...value, ...patch };
    if (!next.directory?.trim()) delete next.directory;
    return next;
}
export function timezones(): string[] {
    let host = "UTC";
    try { host = Intl.DateTimeFormat().resolvedOptions().timeZone || host; } catch { /* SSR */ }
    const intl = Intl as typeof Intl & { supportedValuesOf?: (key: string) => string[] };
    try { return [...new Set(["UTC", host, ...(intl.supportedValuesOf?.("timeZone") ?? [])])].sort(); }
    catch { return [...new Set(["UTC", host])]; }
}
export function isRules(value: unknown): value is Record<string, unknown> { return !!value && typeof value === "object" && !Array.isArray(value); }
export function permissionErrors(permissions: WorkflowDefinition["permissions"]): string[] {
    const errors: string[] = [];
    for (const [name, rule] of Object.entries(permissions ?? {})) {
        const label = `Permission ${name || "(unnamed)"}`;
        if (!name.trim()) errors.push("Permission names must not be empty.");
        if (rule === "allow" || rule === "deny") continue;
        if (rule === "ask") { errors.push(`${label}: scheduled workflows cannot ask. Choose allow/deny or remove it.`); continue; }
        if (!isRules(rule)) { errors.push(`${label}: invalid permission setting. Replace or remove it.`); continue; }
        for (const key of Object.keys(rule)) if (!["default", "allow", "deny", "ask"].includes(key)) errors.push(`${label}: unsupported rule key “${key}”; remove it explicitly.`);
        if (rule.default !== undefined && !["allow", "deny", "ask"].includes(String(rule.default))) errors.push(`${label}: invalid default action.`);
        if (rule.default === "ask" || (Array.isArray(rule.ask) && rule.ask.length)) errors.push(`${label}: scheduled workflows cannot ask. Remove ask rules explicitly.`);
        for (const key of ["allow", "deny", "ask"]) if (rule[key] !== undefined && (!Array.isArray(rule[key]) || (rule[key] as unknown[]).some(p => typeof p !== "string" || !p.trim()))) errors.push(`${label}: ${key} patterns must be non-empty strings.`);
        if (rule.default === undefined && ![rule.allow, rule.deny, rule.ask].some(v => Array.isArray(v) && v.length)) errors.push(`${label}: define a default or at least one pattern.`);
    }
    return errors;
}
const integer = (n: number, min: number, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(n) && n >= min && n <= max;
export function workflowErrors(d: WorkflowDefinition): string[] {
    const errors = permissionErrors(d.permissions);
    const add = (bad: boolean, message: string) => { if (bad) errors.push(message); };
    add(!/^[a-z0-9][a-z0-9._-]*$/.test(d.id), "Workflow ID must be a lowercase slug (letters, numbers, dots, underscores or hyphens).");
    add(!d.name.trim(), "Enter a name."); add(!d.prompt.trim(), "Enter a prompt.");
    const dir = d.directory;
    add(!!dir && !dir.startsWith("/") && !dir.startsWith("~") && !/^[A-Za-z]:[\\/]/.test(dir) && dir.split(/[\\/]/).includes(".."), "Relative execution directories cannot contain '..'. Use an absolute path or ~/….");
    const r = { ...retryDefaults, ...d.retry };
    add(!["fixed", "exponential"].includes(r.backoff), "Choose fixed or exponential retry backoff.");
    add(!integer(r.maxAttempts, 1, 4294967295), "Maximum attempts must be a positive integer.");
    add(!integer(r.initialDelayMs, 0) || !integer(r.maxDelayMs, 0), "Retry delays must be non-negative safe integers.");
    add(r.maxDelayMs > 0 && r.maxDelayMs < r.initialDelayMs, "Maximum retry delay must be 0 (unbounded) or at least the initial delay.");
    add(r.retryableErrors.some(e => !e.trim()), "Retryable error codes cannot be blank.");
    const c = d.concurrency;
    add(!["forbid", "replace", "allow"].includes(c?.mode ?? "forbid"), "Choose forbid, replace or allow concurrency.");
    add(!integer(c?.maxRunning ?? 1, 1, 4294967295), "Maximum running must be a positive integer.");
    add((c?.mode ?? "forbid") !== "allow" && (c?.maxRunning ?? 1) !== 1, "Forbid and replace require maximum running = 1.");
    const s = d.schedule;
    add(!integer(s.interval, 1, 4294967295), "Schedule interval must be a positive integer.");
    if (d.model) add(!d.model.providerId.trim() || !d.model.id.trim(), "Select both a model provider and model ID, or clear the model override.");
    try { if (s.timezone.toLowerCase() !== "local") new Intl.DateTimeFormat("en", { timeZone: s.timezone }).format(); if (!s.timezone.trim()) throw Error(); } catch { errors.push("Choose a valid IANA timezone."); }
    add(!frequencies.includes(s.frequency as typeof frequencies[number]), "Choose a supported schedule frequency.");
    if (s.at !== undefined) {
        const timestampDate = new Date(`${s.at.slice(0, 10)}T00:00:00Z`);
        const validDate = Number.isFinite(+timestampDate) && timestampDate.toISOString().slice(0, 10) === s.at.slice(0, 10);
        add(!/^\d{4}-\d{2}-\d{2}T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/i.test(s.at) || !validDate || !Number.isFinite(Date.parse(s.at)), "Timestamp must be ISO 8601 / RFC 3339 with seconds and a timezone offset.");
        add(s.frequency !== "once" || s.interval !== 1 || s.date !== undefined || s.time !== undefined || s.minute !== undefined || !!s.weekdays?.length || s.monthDay !== undefined, "Timestamp schedules only accept timestamp and timezone (interval 1).");
        return errors;
    }
    if (s.time !== undefined) add(!/^(?:(?:[01]?\d|2[0-3]):[0-5]\d(?::[0-5]\d)?|(?:0?[1-9]|1[0-2]):[0-5]\d(?::[0-5]\d)?\s[AP]M)$/i.test(s.time.trim()), "Time must use HH:MM, HH:MM:SS or h:MM AM/PM.");
    if (s.frequency === "once" || s.date !== undefined) {
        const date = s.date ?? "";
        const parsed = new Date(`${date}T00:00:00Z`);
        add(!/^\d{4}-\d{2}-\d{2}$/.test(date) || !Number.isFinite(+parsed) || (Number.isFinite(+parsed) && parsed.toISOString().slice(0, 10) !== date), "Choose a valid one-time date.");
        add(s.frequency !== "once" || s.interval !== 1 || s.minute !== undefined || !!s.weekdays?.length || s.monthDay !== undefined, "One-time schedules only accept date, time and timezone (interval 1).");
    } else {
        if (s.frequency === "hourly") add(!integer(s.minute ?? 0, 0, 59) || s.time !== undefined || !!s.weekdays?.length || s.monthDay !== undefined, "Hourly schedules require minute 0–59 and no time, weekdays or monthly day.");
        if (s.frequency === "daily") add(s.minute !== undefined || !!s.weekdays?.length || s.monthDay !== undefined, "Daily schedules only accept time.");
        if (s.frequency === "weekly") {
            const days = (s.weekdays ?? []).map(day => day.toLowerCase().slice(0, 3));
            add(!days.length || (s.weekdays ?? []).some(day => !weekdays.some(w => w === day.toLowerCase() || w.slice(0, 3) === day.toLowerCase())) || new Set(days).size !== days.length, "Choose at least one unique, valid weekday.");
            add(s.minute !== undefined || s.monthDay !== undefined, "Weekly schedules only accept time and weekdays.");
        }
        if (s.frequency === "monthly") add(!integer(s.monthDay ?? 1, 1, 31) || s.minute !== undefined || !!s.weekdays?.length, "Monthly schedules require day 1–31 and no minute or weekdays.");
    }
    return errors;
}
/** Cancelling a scope prevents late results (including errors) updating the next workspace. */
export function scopeGuard() {
    let live = true;
    return { cancel: () => { live = false; }, run: async <T,>(work: Promise<T>, success: (value: T) => void, failure: (error: unknown) => void) => { try { const result = await work; if (live) success(result); } catch (error) { if (live) failure(error); } } };
}
export async function loadWorkflowCatalogs(client: NeoismClient, directory?: string) {
    const [agents, skills, configured] = await Promise.all(directory
        ? [client.catalog.agents.list(directory), client.catalog.skills.list(directory), client.catalog.providers.configured(directory)] as const
        : [client.operations.request("v2.agents.list", { query: { scope: "installation" } }), client.operations.request("v2.skills.list", { query: { scope: "installation" } }), client.operations.request("v2.providers.configured", { query: { scope: "installation" } })] as const);
    return { agents: agents.filter(a => !a.hidden && a.mode !== "subagent"), skills, providers: configured.providers };
}
