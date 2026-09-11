import type { MessageWithParts, SessionRuntimeSnapshot } from "@neoism/sdk";

// Ported from shared agent_pane/view/user_input.rs (not the sidebar orbit).
export const ACTIVITY = { scrambleSeconds: 0.7, scrambleHz: 44, scramble: "|/-\\+!?>?<%#=@*~&^$", lineHeight: 26,
    fontSize: 12, inset: 3.3, reserve: 190, minWidth: 72, phaseWrap: 10_000 } as const;
export const STATUS = {
    idle: ["", "idle", "muted"], thinking: ["Pondering", "thinking", "magenta"],
    working: ["Tinkering", "tools", "yellow"], generating: ["Crafting", "reply", "accent"],
    compacting: ["Compacting", "context", "green"], waitingSubagents: ["Sub-agents working", "subagents", "yellow"],
    backgroundTasks: ["Background", "running", "red"], retrying: ["Retrying", "backoff", "yellow"],
} as const;
export type ActivityStatus = keyof typeof STATUS;
export interface NativeActivityState { status: ActivityStatus; retryReason?: string; queuedCount?: number; backgroundCount?: number; elapsedSeconds?: number; palette?: Partial<Record<"bg" | "fg" | "dim" | "muted" | "accent" | "magenta" | "yellow" | "green" | "red", string>> }
export function retryReason(message: string) {
    const compact = message.trim().replace(/\s+/g, " "), lower = compact.toLowerCase();
    for (const [needles, label] of [ [["overloaded", "at capacity"], "Provider overloaded"], [["rate limit", "too many requests", "429"], "Rate limited"],
        [["timed out", "timeout"], "Provider timeout"], [["connection reset", "connection closed", "stream error"], "Connection interrupted"],
        [["service unavailable", "503"], "Service unavailable"] ] as const) if (needles.some(n => lower.includes(n))) return label;
    const chars = [...compact.replace(/[.!?]+$/, "")];
    return chars.length > 72 ? chars.slice(0, 71).join("") + "…" : chars.join("");
}
export function activityLabel(state: NativeActivityState) {
    const reason = state.status === "retrying" && state.retryReason ? retryReason(state.retryReason) : "";
    return STATUS[state.status][0] + (reason ? ` · ${reason}` : "");
}
/** Rust raw_streaming_status precedence: own stream, viewed child, children, jobs, idle.
 * Execution lifetime and aggregate UI busy are NOT evidence of a provider stream. */
export function resolveActivity(messages: MessageWithParts[], busy: boolean, runtime?: SessionRuntimeSnapshot, explicit?: NativeActivityState, sessionId?: string): NativeActivityState {
    const backgroundCount = runtime?.runningBackgroundTasks?.length ?? 0;
    const viewed = sessionId ?? runtime?.rootSessionId;
    const last = messages.at(-1), assistant = last?.info.role === "assistant" && (!viewed || last.info.sessionId === viewed) &&
        typeof last.info.time?.completed !== "number" ? last : undefined;
    const children = runtime?.branches.some(b => b.status === "outstanding" && b.parentSessionId === viewed);
    const child = runtime?.branches.some(b => b.status === "outstanding" && b.sessionId === viewed);
    const execution = runtime?.execution;
    // The server serializes per-session provider activity; older SDK contracts omit it.
    const sessions = (execution as (NonNullable<typeof execution> & { sessionActivities?: Record<string, { activeSegments: Record<string, number> }> }) | undefined)?.sessionActivities;
    const segments = sessions ? sessions[viewed ?? ""]?.activeSegments : execution?.activeSegments;
    const providerActive = !!execution && !execution.finished && !!segments && Object.keys(segments).length > 0;
    // With an older aggregate-only snapshot, use the unaggregated session status to
    // avoid attributing a child's provider segment to an idle root (or vice versa).
    const main = runtime ? providerActive && (sessions ? true : busy) : busy;
    let status: ActivityStatus = "idle";
    if (explicit && explicit.status !== "idle") status = explicit.status;
    else if (main) {
        const part = assistant?.parts.at(-1);
        status = part?.type === "reasoning" && part.time?.end == null ? "thinking" :
            part?.type === "tool" && (part.state.status === "running" || part.state.status === "pending") || part?.type === "subtask" ? "working" : "generating";
    } else if (child) status = "working";
    else if (children) status = "waitingSubagents";
    else if (backgroundCount || explicit?.backgroundCount) status = "backgroundTasks";
    return { backgroundCount, ...explicit, status };
}
export function phaseSeconds(monotonicMs: number) { return (Math.max(0, monotonicMs) / 1000) % ACTIVITY.phaseWrap; }
export function glyphFrame(target: string, index: number, count: number, phase: number, transition: number, reduced = false) {
    const locked = reduced || transition >= (index + 1) * ACTIVITY.scrambleSeconds / Math.max(1, count);
    const wave = phase * 5.6 + index * 0.82;
    const colorWave = Math.sin(phase * (locked ? 3.4 : 8.2) + index * (locked ? 0.62 : 0.91)) * 0.5 + 0.5;
    const pulse = Math.sin(phase * 6.2 + index * 0.9) * 0.5 + 0.5;
    return { char: locked ? target : ACTIVITY.scramble[(Math.floor(phase * 44) + index * 5) % ACTIVITY.scramble.length],
        x: reduced ? 0 : Math.sin(phase * 3) * 1.8 + (locked ? Math.cos(wave) * 1.5 : Math.sin(wave * 1.7) * 0.8),
        y: reduced ? 0 : -(locked ? Math.sin(wave) * 2.4 + Math.cos(phase * 3 * 0.72) * 0.8 : Math.cos(wave * 1.9) * 0.9),
        mix: reduced ? 0 : locked ? 0.06 + colorWave * 0.18 + pulse * 0.08 : 0.12 + colorWave * 0.34 };
}
export function dotFrame(index: number, phase: number, reduced = false) {
    const p = phase * 4 + index * 0.95, swell = Math.sin(p) * 0.5 + 0.5;
    return { x: reduced ? 0 : Math.cos(p * 0.55 + 1.2), y: reduced ? 0 : -swell * 1.7,
        alpha: reduced ? 1 : Math.round((0.40 + swell * 0.45) * 255) / 255 };
}
export function elapsedLabel(seconds: number) {
    const t = Math.max(0, seconds);
    return t < 60 ? `${t.toFixed(1)}s` : t < 3600 ? `${Math.floor(t / 60)}m ${Math.round(t % 60)}s` : `${Math.floor(t / 3600)}h ${Math.floor(t % 3600 / 60)}m`;
}
export function runtimeElapsed(runtime: SessionRuntimeSnapshot | undefined, epochMs: number) {
    const e = runtime?.execution;
    return e ? (e.completedMs + Object.values(e.activeSegments).reduce((sum, start) => sum + Math.max(0, epochMs - start), 0)) / 1000 : undefined;
}
