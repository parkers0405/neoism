import type { Part } from "@neoism/sdk";
import { stripTerminalControls } from "./runtimeMessages";
export type CardPart = Extract<Part, { type: "tool" | "subtask" }>;
export const object = (v: unknown): Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : {};
export const string = (v: unknown): string => typeof v === "string" ? v : "";
export const field = (v: unknown, ...keys: string[]) => keys.map(k => string(object(v)[k])).find(Boolean) || "";
export const clean = (v: unknown, cap = 240) => stripTerminalControls(string(v).slice(0, cap));
export const toolName = (p: CardPart) => p.type === "subtask" ? "task" : p.tool.toLowerCase().replace(/^.*[.:/]/, "");
export const isFileTool = (name: string) => /^(edit|replace_text|replacetext|apply_patch|write|write_file|multiedit)$/.test(name);
export function cardData(part: CardPart) {
    const state = object(part.state);
    const name = toolName(part);
    const partial = name === "task" || name === "task_result" || name === "subtask"
        ? streamedInputFields(string(state.partialRaw) || string(state.raw), ["description", "agent", "subagent_type", "model"]) : {};
    const input = { ...partial, ...object(state.input) }, metadata = { ...object(part.metadata), ...object(state.metadata) };
    const status = part.type === "tool" ? string(state.status) : "requested";
    return { state, input, metadata, status, title: clean(field(state, "title") || field(metadata, "title") || field(input, "description") || (part.type === "subtask" ? part.description : part.tool)), error: string(state.error) };
}
/** Bounded serialization: neither a giant string nor a giant collection is traversed in full. */
export function prettyPreview(value: unknown, budget = 2400): string {
    let remaining = budget;
    const walk = (v: unknown, depth: number): unknown => {
        if (remaining <= 0) return "…";
        if (typeof v === "string") { const size = Math.min(v.length, remaining); remaining -= size; return stripTerminalControls(v.slice(0, size)) + (size < v.length ? "…" : ""); }
        if (v === null || typeof v !== "object") { remaining -= 20; return v; }
        if (depth > 5) return "[nested value]";
        const result: Record<string, unknown> | unknown[] = Array.isArray(v) ? [] : {};
        let n = 0;
        for (const key in v) {
            if (!Object.prototype.hasOwnProperty.call(v, key)) continue;
            if (++n > 40 || remaining <= 0) { if (Array.isArray(result)) result.push("…"); else result["…"] = "More fields omitted from preview"; break; }
            remaining -= key.length;
            const item = walk((v as Record<string, unknown>)[key], depth + 1);
            if (Array.isArray(result)) result.push(item); else Object.defineProperty(result, stripTerminalControls(key), { value: item, enumerable: true });
        }
        return result;
    };
    return typeof value === "string" ? String(walk(value, 0)) : JSON.stringify(walk(value, 0), null, 2) || "";
}
export interface DiffRow { kind: "add" | "remove" | "context" | "hunk"; text: string; partial?: boolean }
export interface FileChange { path: string; action: string; rows: DiffRow[]; truncated: boolean; source: "result" | "input"; patch?: string; moveTo?: string }
const DIFF_CAP = 64000;
/** Decode only a JSON string token, tolerating an unfinished final escape. Never eval/repair JSON. */
function partialString(raw: string, start: number): { value: string; end: number; closed: boolean } {
    let value = "", i = start + 1;
    for (; i < raw.length; i++) {
        const c = raw[i];
        if (c === '"') return { value, end: i + 1, closed: true };
        if (c !== "\\") { value += c; continue; }
        const escape = raw[++i];
        if (!escape) break;
        if (escape === "u") {
            const hex = raw.slice(i + 1, i + 5);
            if (!/^[0-9a-f]{4}$/i.test(hex)) break;
            value += String.fromCharCode(parseInt(hex, 16)); i += 4;
        } else {
            const escapes: Record<string, string> = { n: "\n", r: "\r", t: "\t", b: "\b", f: "\f", '"': '"', "\\": "\\", "/": "/" };
            if (!(escape in escapes)) break;
            value += escapes[escape];
        }
    }
    return { value, end: i, closed: false };
}
/** Builtin patchText plus provider apply_patch aliases. Only top-level string fields qualify;
 * a quoted comment containing `patchText` must never be interpreted as another argument. */
export function streamedPatch(raw: string): string {
    const bounded = raw.slice(0, DIFF_CAP * 6), trimmed = bounded.trimStart();
    if (/^(?:\*\*\* Begin Patch|\*\*\* (?:Update|Add|Delete) File:|diff --git |--- |@@)/.test(trimmed)) return bounded;
    if (trimmed[0] === '"') return partialString(trimmed, 0).value;
    return Object.values(streamedInputFields(trimmed, ["patchText", "patch", "content"]))[0] || "";
}
export function streamedInputFields(raw: string, keys: string[]): Record<string, string> {
    const trimmed = raw.slice(0, DIFF_CAP * 6).trimStart(), fields: Record<string, string> = {};
    if (trimmed[0] !== "{") return fields;
    let depth = 0;
    for (let i = 0; i < trimmed.length; i++) {
        if (trimmed[i] === "{" || trimmed[i] === "[") depth++;
        else if (trimmed[i] === "}" || trimmed[i] === "]") depth--;
        else if (trimmed[i] === '"') {
            const token = partialString(trimmed, i); i = token.end - 1;
            if (!token.closed) break;
            let next = token.end; while (/\s/.test(trimmed[next] || "!") ) next++;
            if (depth !== 1 || trimmed[next] !== ":") continue;
            next++; while (/\s/.test(trimmed[next] || "!")) next++;
            if (keys.includes(token.value) && trimmed[next] === '"') {
                const value = partialString(trimmed, next); fields[token.value] = value.value;
                i = value.end - 1;
                if (!value.closed) break;
            }
        }
    }
    return fields;
}
export function parsePatch(raw: string, path = "File", source: FileChange["source"] = "result", streaming = false): FileChange[] {
    const truncated = raw.length > DIFF_CAP;
    const lines = stripTerminalControls(raw.slice(0, DIFF_CAP)).split("\n");
    if (truncated) lines.pop();
    const files: FileChange[] = []; let current: FileChange | undefined; let inHunk = false, v4a = false;
    let oldRemaining = 0, newRemaining = 0;
    const start = (p: string, action = "update", header = "") => {
        current = { path: p, action, rows: [], truncated, source, patch: header }; files.push(current); inHunk = false;
    };
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i].replace(/\r$/, "");
        const partial = streaming && i === lines.length - 1 && !raw.endsWith("\n");
        const v4 = line.match(/^\*\*\* (Update|Add|Delete) File: (.+)$/);
        if (v4) { start(v4[2], v4[1].toLowerCase(), line + "\n"); inHunk = true; v4a = true; continue; }
        if (line.startsWith("*** Move to: ") && current && v4a) { current.moveTo = line.slice(13); current.action = "move"; current.patch += line + "\n"; continue; }
        if (line.startsWith("diff --git ")) { current = undefined; inHunk = false; v4a = false; continue; }
        if (line.startsWith("--- ") && !v4a && (!inHunk || (oldRemaining === 0 && newRemaining === 0))) { path = line.slice(4).replace(/^a\//, ""); inHunk = false; continue; }
        if (line.startsWith("+++ ") && !inHunk) {
            const next = line.slice(4).replace(/^b\//, "");
            start(next === "/dev/null" ? path : next, path === "/dev/null" ? "add" : next === "/dev/null" ? "delete" : "update", `--- ${path}\n${line}\n`); continue;
        }
        if (line.startsWith("*** End Patch")) { inHunk = false; continue; }
        if (line.startsWith("@@") && (current || /^@@ -\d/.test(line))) {
            if (!current) start(path);
            const hunk = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/);
            oldRemaining = hunk ? Number(hunk[2] ?? 1) : Infinity; newRemaining = hunk ? Number(hunk[4] ?? 1) : Infinity;
            inHunk = true; current!.rows.push({ kind: "hunk", text: line, partial }); current!.patch += line + "\n"; continue;
        }
        if (!inHunk || !current) continue;
        const kind = line[0] === "+" ? "add" : line[0] === "-" ? "remove" : line[0] === " " ? "context" : undefined;
        if (kind) {
            current.rows.push({ kind, text: line.slice(1), partial }); current.patch += line + (partial ? "" : "\n");
            if (kind !== "add") oldRemaining--; if (kind !== "remove") newRemaining--;
        } else if (line.startsWith("\\ No newline")) current.patch += line + "\n";
    }
    return files;
}
function snapshotText(v: unknown): string | undefined {
    if (typeof v === "string") return v;
    const row = object(v);
    if (row.exists === false) return "";
    const encoded = field(row, "contentBase64", "content_base64");
    if (!encoded || encoded.length > DIFF_CAP) return undefined;
    try { return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(atob(encoded), c => c.charCodeAt(0))); } catch { return undefined; }
}
export function fileChanges(part: CardPart): FileChange[] {
    const { state, metadata } = cardData(part);
    if (!isFileTool(toolName(part))) return [];
    const applyPatch = toolName(part) === "apply_patch";
    const streaming = state.status === "pending" || state.status === "running";
    function extract(value: unknown, source: FileChange["source"], depth = 0): FileChange[] {
        if (depth > 5) return [];
        if (typeof value === "string") {
            if (/^\s*[\[{]/.test(value) && value.length <= DIFF_CAP) { try { return extract(JSON.parse(value), source, depth + 1); } catch { /* ordinary text */ } }
            if (source === "input" && !applyPatch) return [];
            return parsePatch(applyPatch && source === "input" ? streamedPatch(value) : value, "File", source, streaming && source === "input");
        }
        if (Array.isArray(value)) return value.slice(0, 40).flatMap(v => extract(v, source, depth + 1));
        const row = object(value), path = field(row, "relativePath", "filePath", "file_path", "path") || "File";
        for (const key of ["files", "snapshots", "diffs", "edits"]) { if (row[key]) { const found = extract(row[key], source, depth + 1); if (found.length) return found; } }
        // `content` is patch text only for apply_patch, never for a generic write payload.
        for (const key of (source === "input" ? applyPatch ? ["patchText", "patch", "content"] : [] : ["patch", "diff"])) {
            if (typeof row[key] === "string") {
                const found = parsePatch(row[key], path, source, streaming && source === "input");
                if (found.length) return found.map(change => ({ ...change, action: field(row, "type", "kind") || change.action }));
            }
        }
        const old = snapshotText(row.before ?? row.oldString ?? row.oldText ?? row.old), next = snapshotText(row.after ?? row.newString ?? row.newText ?? row.new);
        // Both sides must be supplied; write content alone is not proof of an added file.
        if (old === undefined || next === undefined) return [];
        const truncated = old.length > DIFF_CAP / 2 || next.length > DIFF_CAP / 2;
        const split = (s: string) => s === "" ? [] : s.replace(/\n$/, "").split("\n");
        const a = split(old.slice(0, DIFF_CAP / 2)), b = split(next.slice(0, DIFF_CAP / 2));
        let prefix = 0, suffix = 0;
        while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++;
        while (suffix + prefix < a.length && suffix + prefix < b.length && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++;
        const rows: DiffRow[] = [...a.slice(Math.max(0, prefix - 3), prefix).map(text => ({ kind: "context" as const, text })), ...a.slice(prefix, a.length - suffix).map(text => ({ kind: "remove" as const, text })), ...b.slice(prefix, b.length - suffix).map(text => ({ kind: "add" as const, text })), ...b.slice(b.length - suffix, b.length - suffix + 3).map(text => ({ kind: "context" as const, text }))];
        return [{ path, action: field(row, "type", "kind") || "update", rows: rows.map(r => ({ ...r, text: stripTerminalControls(r.text) })), truncated, source }];
    }
    // While arguments stream, raw is newer than a partially materialized input object.
    const inputs = applyPatch && streaming ? [state.partialRaw, state.raw, state.input] : [state.input, ...(applyPatch ? [state.raw, state.partialRaw] : [])];
    for (const [value, source] of [[metadata, "result"], [state.metadata, "result"], [state.output, "result"], ...inputs.map(value => [value, "input"] as const)] as const) {
        const changes = extract(value, source); if (changes.length) return changes;
    }
    return [];
}
export type TaskChildStatus = "outstanding" | "completed" | "failed" | "unknown";
/** A current runtime branch supersedes the task tool's historical metadata. */
export function taskActivityStatus(part: CardPart, childStatus?: TaskChildStatus): string {
    const { status, metadata } = cardData(part);
    if (status === "error") return "error";
    if (childStatus === "outstanding") return "running";
    if (childStatus === "completed") return "completed";
    if (childStatus === "failed") return "error";
    if (childStatus === "unknown") return status; // no evidence to extend a completed call's animation
    const child = string(metadata.status);
    if (["failed", "error"].includes(child)) return "error";
    if (["completed", "cancelled", "stopped"].includes(child)) return child;
    if (["pending", "running", "outstanding"].includes(child)) return child === "outstanding" ? "running" : child;
    return status;
}
export function taskIdentity(part: CardPart) {
    const { state, metadata, input } = cardData(part);
    const taskId = field(metadata, "taskId", "task_id");
    // Runtime task IDs are not universally session IDs. Never navigate to the parent part.sessionId.
    const output = string(state.output).slice(0, 8000);
    const legacy = output.match(/^task_id:\s*(ses_[\w-]+)/m)?.[1] || "";
    return { taskId, sessionId: field(metadata, "childSessionId", "sessionId", "sessionID", "session_id", "sessID") || legacy,
        agent: clean(field(metadata, "agent") || field(input, "subagent_type", "agent") || (part.type === "subtask" ? part.agent : "Agent")),
        model: clean(field(metadata, "model") || field(input, "model") || field(part.model, "modelID", "modelId")),
        description: clean(field(input, "description") || field(metadata, "description") || (part.type === "subtask" ? part.description : "")) };
}
