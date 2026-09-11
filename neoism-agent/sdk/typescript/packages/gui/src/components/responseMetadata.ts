// Message-owned presentation data; never consult composer/provider selection.
const object = (v: unknown): Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : {};
const string = (v: unknown): string => typeof v === "string" ? v.trim() : "";
const uint = (v: unknown): number | undefined => typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : undefined;
const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
export const displayAgent = (s: string) => s.split(/[-_\s]+/).filter(Boolean).map(capital).join(" ");
export function displayModel(value: string): string {
    const name = value.split("/").at(-1) ?? "";
    if (/^(gpt|GPT)-/.test(name)) {
        const [version, ...suffix] = name.slice(4).split(/[-_]+/).filter(Boolean);
        return version ? `GPT-${version}${suffix.length ? ` ${suffix.map(capital).join(" ")}` : ""}` : "GPT";
    }
    return name.split(/[-_\s]+/).filter(Boolean).map(p => /^(gpt|api|ai)$/i.test(p) ? p.toUpperCase() : capital(p)).join(" ");
}
export function displayDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
    if (ms < 3600000) return `${Math.floor(ms / 60000)}m ${Math.floor(ms % 60000 / 1000)}s`;
    if (ms < 86400000) return `${Math.floor(ms / 3600000)}h ${Math.floor(ms % 3600000 / 60000)}m`;
    return `${Math.floor(ms / 86400000)}d ${Math.floor(ms % 86400000 / 3600000)}h`;
}
export function parseAssistant(value: unknown) {
    const i = object(value), time = object(i.time);
    if (i.role !== "assistant") return undefined;
    // Optional legacy fields may be absent, but malformed wire fields are not metadata.
    for (const key of ["agent", "mode", "modelId", "providerId", "parentId", "finish"]) {
        if (i[key] !== undefined && typeof i[key] !== "string") return undefined;
    }
    const agent = string(i.agent) || string(i.mode), model = string(i.modelId);
    const created = uint(time.created);
    if (!agent || !model || created === undefined) return undefined;
    return { agent, model, provider: string(i.providerId), parent: string(i.parentId), created,
        completed: uint(time.completed), streamed: uint(time.streamed), output: uint(object(i.tokens).output),
        finish: string(i.finish), terminalError: i.error !== undefined && i.error !== null };
}
const taskIds = (value: unknown) => string(value).split("\n").flatMap(line => {
    const match = /^task_id:\s*(\S+)/.exec(line.trim());
    return match ? [match[1]] : [];
});
/** Chronological transcript, just like Timeline. Missing history must not manufacture a rate. */
export function responseMetadata(messages: readonly unknown[]): (string | undefined)[] {
    const rows = messages.map(m => { const row = object(m); return { info: object(row.info), parts: Array.isArray(row.parts) ? row.parts.map(object) : [] }; });
    const assistants = rows.map(row => parseAssistant(row.info));
    const users = new Map<string, number>();
    for (const { info } of rows) {
        const created = uint(object(info.time).created);
        if (info.role === "user" && string(info.id) && created !== undefined) users.set(string(info.id), created);
    }
    // Native runtime_origins: task tool -> child session -> synthetic completion user.
    const tasks = new Map<string, number>(), origins = new Map<string, number>();
    rows.forEach((row, index) => {
        const parent = assistants[index]?.parent, start = parent ? users.get(parent) : undefined;
        if (start === undefined) return;
        for (const part of row.parts) {
            if (part.type !== "tool" || part.tool !== "task") continue;
            const state = object(part.state);
            const ids = [object(state.metadata), object(part.metadata)].map(m => string(m.sessionId ?? m.sessionID ?? m.session_id));
            const id = ids.find(Boolean) || taskIds(state.output)[0];
            if (id) tasks.set(id, start);
        }
    });
    for (const row of rows) {
        if (row.info.role !== "user" || !string(row.info.system).includes("runtime notification: background subagent completion.")) continue;
        const starts = row.parts.flatMap(p => taskIds(p.text)).flatMap(id => tasks.has(id) ? [tasks.get(id)!] : []);
        if (starts.length) origins.set(string(row.info.id), Math.min(...starts));
    }
    let turn = -1, output = 0, streamMs = 0, rateValid = true;
    return rows.map((row, index) => {
        if (row.info.role === "user") { turn = index; output = 0; streamMs = 0; rateValid = true; return undefined; }
        if (row.info.role !== "assistant") return undefined;
        const a = assistants[index];
        if (!a || a.streamed === undefined || a.output === undefined || turn < 0 || !users.has(a.parent)) rateValid = false;
        if (a && a.streamed !== undefined && a.output !== undefined) {
            output += a.output; streamMs += Math.max(0, a.streamed - a.created);
        }
        if (!a || a.completed === undefined || !row.parts.some(p => p.type === "text")) return undefined;
        if (["tool-calls", "unknown"].includes(a.finish) && !a.terminalError) return undefined;
        const agent = displayAgent(a.agent), model = displayModel(a.model);
        if (!agent || !model) return undefined;
        const start = origins.get(a.parent) ?? users.get(a.parent) ?? a.created;
        const rate = rateValid && output > 0 && streamMs > 0 && Number.isFinite(output / streamMs) ? ` · ${(output * 1000 / streamMs).toFixed(1)} tok/s` : "";
        return `${agent} · ${model} · ${displayDuration(Math.max(0, a.completed - start))}${rate}`;
    });
}
