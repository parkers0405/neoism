import { useState, type ReactNode } from "react";
import { ToolTreePreview } from "./ToolTreePreview";
import "./runtime-messages.css";

const record = (value: unknown): Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const text = (value: unknown): string => typeof value === "string" ? value : "";
const prefixes = { shell: "msg_background_completion_", subagent: "msg_subtask_completion_" } as const;
export type RuntimeKind = keyof typeof prefixes;
export interface RuntimeMessage {
    kind: RuntimeKind; id: string; title: string; status: string;
    fields: Record<string, string>; output: string; envelope: string;
}
/** Remove terminal controls, never interpret HTML or terminal hyperlinks. Keep whitespace/output. */
export function stripTerminalControls(value: string): string {
    return value.replace(/(?:\x1b\]|\x9d)[\s\S]*?(?:\x07|\x1b\\|\x9c|$)/g, "")
        .replace(/\x1b[P^_X][\s\S]*?(?:\x1b\\|$)/g, "")
        .replace(/(?:\x1b\[|\x9b)[0-?]*[ -/]*[@-~]/g, "")
        .replace(/\x1b[ -/]*[@-~]/g, "")
        .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/g, "");
}
function bodyOf(value: unknown): { info: Record<string, unknown>; parts: unknown[]; source: string; id: string } {
    const row = record(value), info = record(row.info);
    const parts = Array.isArray(row.parts) ? row.parts : row.type === "text" ? [row] : [];
    const chunks = parts.filter(p => record(p).type === "text").map(p => text(record(p).text));
    // Parts can be separate paragraphs (title, envelope) or contiguous fragments.
    // Prefer the assembly that preserves protocol line boundaries, not arbitrary
    // spaces/newlines inside values or streamed fragments.
    const candidates = [chunks.join(""), chunks.join("\n")];
    const score = (s: string) => (s.match(/^(?:job_id|task_id|description|status|exit_code|cwd|command|agent|title|count):|^(?:Subagents? finished\.|Background shell task finished\.)|^<(?:background_task|task)_(?:result|error)>/gm) || []).length;
    const source = score(candidates[1]) > score(candidates[0]) ? candidates[1] : candidates[0];
    return { info, parts, source,
        id: text(info.id) || text(row.messageID) || text(row.messageId) || parts.map(p => text(record(p).messageID) || text(record(p).messageId)).find(Boolean) || "" };
}
/** Decode both persisted MessageWithParts and early message.part.updated text parts.
 * Text fallback requires the actual envelope structure, not a phrase in human prose.
 * Reserved IDs/system metadata classify even an empty first delta, avoiding bubble flicker.
 */
export function decodeRuntimeMessage(value: unknown): RuntimeMessage | undefined {
    const { info, source, id } = bodyOf(value);
    const system = text(info.system) || text(record(value).system);
    // message_model.rs deliberately matches the notification kind, not the
    // product prefix: current servers emit Agent; older histories emit Neoism.
    let kind: RuntimeKind | undefined = system.includes("runtime notification: background shell task completion.") || id.startsWith(prefixes.shell) ? "shell"
        : system.includes("runtime notification: background subagent completion.") || id.startsWith(prefixes.subagent) ? "subagent" : undefined;
    const identified = kind !== undefined;
    if (!identified && info.role !== undefined && info.role !== "user" && info.role !== "system") return undefined;
    const clean = stripTerminalControls(source).trimStart();
    // A literal transport heading alone is not proof: still require a complete,
    // anchored envelope below it. Never classify an ordinary mention/quotation.
    const fallback = clean.replace(/^(?:(?:Agent|Neoism) )?runtime notification: background (?:shell task|subagent) completion\.\r?\n\s*/, "");
    // Anchored, ordered headers + result tag distinguish quotes/mentions. Older
    // notifications may omit the explanatory context sentence.
    if (!kind && /^Background shell task finished\.\r?\njob_id: \S+\r?\ndescription: [\s\S]*?\r?\nstatus: (?:completed|error|cancelled|timed_out|running)\r?\nexit_code: [^\n]+\r?\ncwd: [^\n]+\r?\ncommand: [^\n]+(?:\r?\n|$)/.test(fallback)
        && /\n<background_task_(?:result|error)>\r?\n/.test(fallback)) kind = "shell";
    if (!kind && /^Subagent finished\.\r?\ntask_id: \S+\r?\nagent: @[^\n]+\r?\ntitle: [^\n]*\r?\nstatus: [^\n]+\r?\n/.test(fallback)
        && clean.includes("The subagent result is included below as runtime system context.") && /\n<task_(?:result|error)>\r?\n/.test(fallback)) kind = "subagent";
    if (!kind && /^Subagents finished\.\r?\ncount: \d+\r?\n/.test(fallback)
        && clean.includes("The subagent results are included below as runtime system context.") && /\ntask_id: \S+/.test(fallback) && /\n<task_(?:result|error)>\r?\n/.test(fallback)) kind = "subagent";
    if (!kind) return undefined;
    const opening = /(?:^|\n)<(background_task_result|background_task_error|task_result|task_error)>\r?\n/.exec(clean);
    const header = opening ? clean.slice(0, opening.index) : clean;
    const fields: Record<string, string> = {};
    for (const line of header.split(/\r?\n/)) {
        const match = /^(job_id|task_id|description|status|exit_code|cwd|command|agent|title|count): ?(.*)$/.exec(line);
        if (match && !(match[1] in fields)) fields[match[1]] = match[2];
    }
    const taskId = fields[kind === "shell" ? "job_id" : "task_id"] || (id.startsWith(prefixes[kind]) ? id.slice(prefixes[kind].length) : "");
    let output = "";
    if (opening) {
        const start = opening.index + opening[0].length, end = clean.lastIndexOf(`\n</${opening[1]}>`);
        output = clean.slice(start, end >= start ? end : undefined);
    }
    // Batched child results stay intact in details; do not silently discard later children.
    if (fields.count) output = clean;
    // session_actions.rs constructs child.title as description + this suffix.
    // Strip only in confirmed runtime presentation; original fields/envelope stay
    // available in details, and ordinary user/assistant text is never rewritten.
    const leadingTitle = identified && kind === "subagent"
        ? header.split(/\r?\n/).find(line => /^.+ \(@[^\s()]+ subagent\)$/.test(line.trim()))?.trim() : undefined;
    const rawTitle = fields.description || fields.title || leadingTitle || (kind === "shell" ? "Background shell task" : "Subagent finished");
    const title = kind === "subagent" ? rawTitle.replace(/ \(@[^\s()]+ subagent\)$/, "").trim() || "Subagent finished" : rawTitle;
    return { kind, id: fields.count ? id : taskId ? `${kind === "shell" ? "background-task" : "subtask"}-${taskId}` : id,
        title: fields.count ? `${fields.count} subagents finished` : title,
        status: fields.status || "completed", fields, output, envelope: clean };
}
/** Index-preserving projection: metadata/scrolling continue to use original messages.
 * Ordinary messages and every non-envelope part retain their identity. Never mutate wire data.
 */
export function normalizeMessages<T>(messages: readonly T[]): { message: T; runtime?: RuntimeMessage; remainingParts: unknown[] }[] {
    return messages.map(message => {
        const runtime = decodeRuntimeMessage(message), { parts } = bodyOf(message);
        return { message, runtime, remainingParts: runtime ? parts.filter(part => record(part).type !== "text") : parts };
    });
}
export function RuntimeNotice({ notice, children }: { notice: RuntimeMessage; children?: ReactNode }) {
    const [expanded, setExpanded] = useState(false);
    const [limit, setLimit] = useState(8000);
    const status = ["completed", "running", "cancelled", "error", "timed_out"].includes(notice.status) ? notice.status : "unknown";
    const preview = notice.output.split("\n").map(line => line.trim()).filter(Boolean).slice(0, 2).join(" ").slice(0, 180)
        || (status === "running" || status === "unknown" ? "Waiting for task output…" : "No output returned.");
    return <section className={`neo-runtime-notice tc-card ${status}`} aria-label={notice.kind === "shell" ? "Background task completion" : "Subagent completion"}>
        <button type="button" className="neo-runtime-toggle" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>
            <span className="neo-runtime-dot" aria-hidden="true" /><span className="neo-runtime-title">{notice.title}</span><small>{notice.status.replaceAll("_", " ")}</small>
        </button>
        {!expanded && <ToolTreePreview text={preview} toggle={() => setExpanded(true)} />}
        {expanded && <div className="tc-tree-body"><pre aria-label="Task output" onScroll={event => {
            const node = event.currentTarget;
            if (limit < notice.output.length && node.scrollHeight - node.scrollTop - node.clientHeight < 80) setLimit(value => value + 8000);
        }}><code>{notice.output.slice(0, limit) || preview}</code></pre></div>}
        {children}
    </section>;
}
