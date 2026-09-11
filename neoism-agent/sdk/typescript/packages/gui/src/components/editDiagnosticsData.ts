import { cardData, fileChanges, isFileTool, object, string, toolName, type CardPart } from "./toolCardData";
import { stripTerminalControls } from "./runtimeMessages";

export interface EditDiagnostic {
    severity: "error" | "warning";
    line: number;
    column: number;
    message: string;
    key: string;
}
export interface EditDiagnosticFile { path: string; cached: boolean; diagnostics: EditDiagnostic[] }
const text = (value: unknown) => stripTerminalControls(string(value));

function normalizedPath(value: string) {
    const windows = /^[a-z]:[\\/]|^\\\\/i.test(value) || value.includes("\\");
    const raw = value.replace(/\\/g, "/");
    const absolute = raw.startsWith("/") || /^[a-z]:\//i.test(raw);
    const parts: string[] = [];
    for (const part of raw.split("/")) {
        if (!part || part === ".") continue;
        if (part === ".." && parts.length && parts.at(-1) !== "..") parts.pop();
        else parts.push(part);
    }
    return { path: parts.join("/"), absolute, windows };
}
/** Suffixes must start at a path component, and only bridge relative/absolute paths.
 * Do not conflate two absolute project roots or case-fold POSIX filenames. */
export function editDiagnosticPathMatches(a: string, b: string): boolean {
    const left = normalizedPath(a), right = normalizedPath(b);
    let x = left.path, y = right.path;
    if (!x || !y) return false;
    if (left.windows || right.windows) { x = x.toLowerCase(); y = y.toLowerCase(); }
    if (left.absolute === right.absolute) return x === y;
    const absolute = left.absolute ? x : y, relative = left.absolute ? y : x;
    return absolute === relative || absolute.endsWith(`/${relative}`);
}
const coordinate = (v: unknown) => typeof v === "number" && Number.isSafeInteger(v) && v >= 0 && v < Number.MAX_SAFE_INTEGER ? v + 1 : 1;
function primary(value: unknown): EditDiagnostic | undefined {
    const item = object(value);
    // Unknown severity is not evidence of an error. Missing severity follows native's default.
    const severity = item.severity == null ? "error" : item.severity;
    if (severity !== "error" && severity !== "warning") return;
    const message = text(item.message).trim();
    if (!message) return;
    const range = object(item.range), start = object(range.start), end = object(range.end);
    const line = coordinate(start.line), column = coordinate(start.character);
    const code = typeof item.code === "number" || typeof item.code === "string" ? String(item.code) : "";
    const key = JSON.stringify([severity, line, column, coordinate(end.line), coordinate(end.character), code, message]);
    return { severity, line, column, message, key };
}
const CACHED_PREFIX = "Cached LSP errors (may predate this edit; verify before fixing):";
function legacyEntries(output: unknown): unknown[] {
    let body = text(output).replace(/\r\n/g, "\n").trim();
    // Native appends this exact, line-anchored report after its success text.
    const lines = body.split("\n"), marker = lines.indexOf(CACHED_PREFIX);
    if (marker >= 0) body = lines.slice(marker + 1).join("\n").trim();
    // Otherwise accept ONLY a complete standalone report, never tags inside code/prose.
    const entries: unknown[] = [];
    while (body) {
        const block = /^<diagnostics file="([^"\n]+)">\n([\s\S]*?)\n<\/diagnostics>(?:\n|$)/.exec(body);
        if (!block) return [];
        const diagnostics: Record<string, unknown>[] = [];
        let current: Record<string, unknown> | undefined;
        for (const line of block[2].split("\n")) {
            if (/^\.\.\. and \d+ more$/.test(line)) { current = undefined; continue; }
            const row = /^(ERROR|WARN|INFO|HINT) \[([1-9]\d*):([1-9]\d*)\] (.+)$/.exec(line);
            if (row) {
                if (!Number.isSafeInteger(Number(row[2])) || !Number.isSafeInteger(Number(row[3]))) return [];
                current = { severity: ({ ERROR: "error", WARN: "warning", INFO: "information", HINT: "hint" })[row[1]], message: row[4], range: { start: { line: Number(row[2]) - 1, character: Number(row[3]) - 1 } } };
                diagnostics.push(current);
            } else {
                // Native pretty() preserves multiline messages verbatim. Continuations
                // qualify only inside a complete report after a valid primary row.
                if (!current || /^(?:ERROR|WARN|INFO|HINT)\b|^<\/?diagnostics\b/.test(line)) return [];
                current.message = string(current.message) + "\n" + line;
            }
        }
        // Decode attribute entities only, never interpret diagnostic messages as HTML.
        const path = block[1].replace(/&(?:quot|apos|lt|gt|amp);/g, entity => ({ "&quot;": '"', "&apos;": "'", "&lt;": "<", "&gt;": ">", "&amp;": "&" })[entity]!);
        entries.push({ path, diagnosticsKind: "cached", diagnostics });
        body = body.slice(block[0].length).trim();
    }
    return entries;
}

/** No LSP requests. Explicit metadata (including empty arrays) always beats legacy output.
 * paths augments derived file changes/input, including rename destinations/source paths. */
export function readEditDiagnostics(part: CardPart, paths: string[] = []): EditDiagnosticFile[] {
    if (!isFileTool(toolName(part))) return [];
    const { input, metadata, state } = cardData(part);
    const edited = [...paths, ...fileChanges(part).flatMap(change => [change.path, change.moveTo]), input.filePath, input.path, input.file_path, input.moveTo, input.sourcePath]
        .filter((path): path is string => typeof path === "string" && path.length > 0 && path !== "File");
    const entries = Object.prototype.hasOwnProperty.call(metadata, "diagnostics")
        ? Array.isArray(metadata.diagnostics) ? metadata.diagnostics : [] : legacyEntries(state.output);
    const files: EditDiagnosticFile[] = [];
    for (const value of entries) {
        const entry = object(value), path = string(entry.path);
        if (!path || !edited.some(editedPath => editDiagnosticPathMatches(editedPath, path)) || !Array.isArray(entry.diagnostics)) continue;
        let file = files.find(file => editDiagnosticPathMatches(file.path, path));
        if (!file) { file = { path, cached: false, diagnostics: [] }; files.push(file); }
        file.cached ||= entry.diagnosticsKind === "cached" || entry.freshness !== "current";
        const seen = new Set(file.diagnostics.map(item => item.key));
        for (const value of entry.diagnostics) {
            const diagnostic = primary(value);
            if (diagnostic && !seen.has(diagnostic.key)) { seen.add(diagnostic.key); file.diagnostics.push(diagnostic); }
        }
    }
    return files.filter(file => file.diagnostics.length).map(file => ({ ...file, path: text(file.path), diagnostics: file.diagnostics.sort((a, b) => Number(b.severity === "error") - Number(a.severity === "error")) }));
}
