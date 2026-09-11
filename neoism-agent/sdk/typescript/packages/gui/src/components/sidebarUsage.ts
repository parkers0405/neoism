import type { NeoismClient, StepFinishPart } from "@neoism/sdk";
import { activeTaskStatus, finite, record, type TaskRow } from "./chatSupport";

export type SidebarCatalog = Awaited<ReturnType<NeoismClient["catalog"]["providers"]["list"]>>;
/** The controller owns catalog loading; unknown capacity stays unknown. */
export type SidebarUsageSource = { providerCatalog?: SidebarCatalog };

export function rawContextLimit(catalog: SidebarCatalog | undefined, model: string): number | undefined {
    const slash = model.indexOf("/");
    if (slash < 1) return;
    const provider = catalog?.all.find(p => p.id === model.slice(0, slash));
    const id = model.slice(slash + 1);
    const entry = provider?.models[id] || Object.values(provider?.models || {}).find(m => m.id === id);
    const limit = finite(record(entry?.limit).context);
    return limit > 0 ? limit : undefined;
}

/** Input is chronological, like the controller's message/part list. Never lifetime totals. */
export function latestSidebarUsage(parts: readonly StepFinishPart[]) {
    const cost = finite(parts.at(-1)?.cost);
    for (let i = parts.length - 1; i >= 0; i--) {
        const tokens = record(parts[i].tokens), cache = record(tokens.cache);
        const input = finite(tokens.input), output = finite(tokens.output), reasoning = finite(tokens.reasoning);
        const cacheRead = finite(cache.read), cacheWrite = finite(cache.write);
        // Native context policy deliberately ignores provider total, including reasoning separately.
        const context = input + output + reasoning + cacheRead + cacheWrite;
        if (context > 0) return { context, input, output, reasoning, cacheRead, cacheWrite, cost };
    }
    return { context: undefined, input: 0, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0, cost };
}

export function contextFraction(context: number | undefined, limit: number | undefined): number | undefined {
    if (context === undefined || !Number.isFinite(context) || !limit || !Number.isFinite(limit) || limit <= 0) return;
    return Math.max(0, Math.min(1, context / limit));
}
export function sidebarTokenCount(value: number): string {
    return value >= 10000 ? `${(value / 1000).toFixed(1)}k` : Math.round(value).toLocaleString("en-US");
}
export function contextCaption(context: number | undefined, limit: number | undefined): string {
    return `${context === undefined ? "—" : sidebarTokenCount(context)} / ${limit && Number.isFinite(limit) && limit > 0 ? sidebarTokenCount(limit) : "—"} tokens`;
}
export function activeSidebarTasks(rows: readonly TaskRow[], parentId?: string): TaskRow[] {
    return rows.filter(row => row.sessionId !== parentId && activeTaskStatus(row.status));
}
export function sidebarTaskTitle(task: TaskRow): string {
    if (!task.title || task.title === task.sessionId || task.title === task.id) return "Subagent";
    return task.title.replace(/\s+\(@[^()]+\s+subagent\)\s*$/, "").trim() || "Subagent";
}
