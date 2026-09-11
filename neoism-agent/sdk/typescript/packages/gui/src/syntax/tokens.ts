/** Palette contract mirrored from shared/src/syntax.rs::tree_sitter_capture_kind/syn_color.
 * Native code spans change color only: no added bold/italic styling. */
export type Token = "plain" | "keyword" | "type" | "string" | "number" | "comment" | "func" | "property" | "constructor" | "special";
export interface Span { start: number; end: number; token: Token }
export const MAX_SOURCE = 64 * 1024; // UTF-16 units; reject before loading any assets
const aliases: Record<string, string> = {
    rust: "rust", rs: "rust", javascript: "javascript", js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
    typescript: "typescript", ts: "typescript", tsx: "tsx", python: "python", py: "python",
    bash: "bash", sh: "bash", shell: "bash", zsh: "bash", json: "json", css: "css", html: "html", htm: "html",
};
export function languageName(name: string): string | undefined { return aliases[name.toLowerCase()]; }
export function captureToken(name: string): Token {
    if (name.startsWith("comment")) return "comment";
    if (["string.escape", "string.regex", "string.regexp", "escape", "label", "character.special", "constructor"].includes(name)) return "constructor";
    if (["string", "string.special", "symbol", "character"].includes(name)) return "string";
    if (["number", "number.float", "float", "boolean", "constant", "constant.builtin", "variable.builtin", "variable.super"].includes(name)) return "number";
    if (["keyword.type", "type", "type.builtin", "type.definition", "type.qualifier", "tag", "attribute", "attribute.builtin"].includes(name)) return "type";
    if (name === "keyword" || name.startsWith("keyword.")) return "keyword";
    if (["function.macro", "constant.macro", "macro", "module", "module.builtin", "namespace", "property", "field", "variable.member", "variable.member.key", "variable.parameter", "variable.parameter.builtin", "tag.attribute"].includes(name)) return "property";
    if (name === "function" || name.startsWith("function.") || name === "method") return "func";
    if (name === "annotation" || name.startsWith("punctuation") || name === "tag.delimiter") return "special";
    return "plain"; // operators and ordinary variables intentionally use native foreground
}
export interface Capture { start: number; end: number; name: string; pattern: number }
/** Sweep overlapping captures. Nested captures override enclosing strings; for identical
 * nodes semantic captures beat broad variable/property rules, then later patterns win.
 * Produces contiguous UTF-16 spans, including unhighlighted gaps, never duplicate text. */
export function resolveCaptures(length: number, captures: Capture[]): Span[] {
    const priority = (c: Capture) => c.name === "variable" ? 0 : c.name === "property" ? 1 : 2;
    const valid = captures.filter(c => c.start >= 0 && c.end <= length && c.end > c.start);
    const events = new Map<number, { add: number[]; remove: number[] }>();
    const at = (i: number) => { let e = events.get(i); if (!e) { e = { add: [], remove: [] }; events.set(i, e); } return e; };
    at(0); at(length);
    valid.forEach((c, i) => { at(c.start).add.push(i); at(c.end).remove.push(i); });
    const active = new Set<number>(), spans: Span[] = [];
    let previous = 0;
    for (const [position, event] of [...events].sort((a, b) => a[0] - b[0])) {
        if (position > previous) {
            let winner: Capture | undefined;
            for (const i of active) {
                const c = valid[i];
                if (!winner || c.end - c.start < winner.end - winner.start ||
                    (c.end - c.start === winner.end - winner.start && (priority(c) > priority(winner) ||
                        (priority(c) === priority(winner) && c.pattern >= winner.pattern)))) winner = c;
            }
            const token = winner ? captureToken(winner.name) : "plain";
            const last = spans.at(-1);
            if (last?.token === token) last.end = position; else spans.push({ start: previous, end: position, token });
        }
        event.remove.forEach(i => active.delete(i)); event.add.forEach(i => active.add(i)); previous = position;
    }
    return spans;
}
