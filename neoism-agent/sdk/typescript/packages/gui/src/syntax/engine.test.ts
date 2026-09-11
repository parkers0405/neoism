// @vitest-environment node
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { SyntaxEngine } from "./engine";
import { captureToken, MAX_SOURCE, resolveCaptures } from "./tokens";
const assets = new URL("../../public/syntax/", import.meta.url);
const engine = new SyntaxEngine({
    runtime: fileURLToPath(new URL("../../../../node_modules/web-tree-sitter/tree-sitter.wasm", import.meta.url)),
    grammar: async name => new Uint8Array(await readFile(new URL(`${name}.wasm`, assets))),
    query: name => readFile(new URL(`${name}.scm.txt`, assets), "utf8"),
});
const samples: Record<string, string> = {
    rust: 'fn main() { let café = "😀hello\\n"; println!("ok"); /* comment\ncontinued */ let n = 42; }',
    bash: '# comment\nif true; then echo "hello $USER"; exit 42 2>/dev/null; fi',
    javascript: '// comment\nconst x = "😀hello"; function run(arg) { return arg + 42; } run(x);',
    typescript: 'interface User { name: string }\nconst value = client.fetch<User>(url, true);',
    tsx: 'const view = <Widget title="hello">text</Widget>;',
    python: '# comment\ndef run(arg):\n    return "hello" + str(42)',
    json: '{ "hello": 42, "ok": true }',
    css: '/* comment */ body { color: red; margin: 42px; }',
    html: '<!-- comment --> <div class="hello">text</div>',
};
describe("real vendored Tree-sitter WASMs", () => {
    for (const [language, source] of Object.entries(samples)) it(`parses and queries ${language}`, async () => {
        const spans = await engine.highlight(source, language);
        expect(spans.length).toBeGreaterThan(2);
        expect(spans.map(s => source.slice(s.start, s.end)).join("")).toBe(source);
        expect(spans.some(s => s.token !== "plain")).toBe(true);
    });
    for (const language of ["rust", "bash", "javascript"]) it(`${language} captures native literal/comment categories`, async () => {
        const spans = await engine.highlight(samples[language], language);
        for (const token of ["comment", "string", "keyword", "number", "func"]) {
            expect(spans.some(s => s.token === token), `${language}: ${token}`).toBe(true);
        }
    });
    it("keeps contextual functions, members, escapes and Unicode boundaries", async () => {
        const cases = [
            { language: "rust", source: 'fn run(arg: String) { arg.field; let s = "😀\\\\n"; }', expected: { run: "func", arg: "property", field: "property", String: "type" } },
            { language: "ts", source: 'const value = client.fetch<User>(url, true);', expected: { fetch: "func", User: "type", true: "number" } },
            { language: "jsx", source: 'const node = <div title="😀">hello</div>;', expected: { div: "type", title: "type" } },
        ];
        for (const { language, source, expected } of cases) {
            const spans = await engine.highlight(source, language);
            for (const [text, token] of Object.entries(expected)) {
                const index = source.indexOf(text);
                expect(spans.find(s => s.start <= index && s.end > index)?.token, `${language}: ${text}`).toBe(token);
            }
        }
    });
    it("preserves UTF-16 and incomplete streaming code", async () => {
        for (const source of ['const x = "😀é"; // next', 'fn main() { let s = "unfinished', 'echo "hello ${USER']) {
            const spans = await engine.highlight(source, source.startsWith("fn") ? "rust" : source.startsWith("echo") ? "bash" : "js");
            expect(spans.map(s => source.slice(s.start, s.end)).join("")).toBe(source);
        }
    });
    it("unsupported and oversized code needs no assets", async () => {
        const unloaded = new SyntaxEngine({ runtime: "missing", grammar: async () => { throw Error("must not load"); }, query: async () => "" });
        expect(await unloaded.highlight("<unsafe>", "unknown")).toEqual([]);
        expect(await unloaded.highlight("x".repeat(MAX_SOURCE + 1), "rust")).toEqual([]);
    });
});
it("native mappings distinguish context and leave operators/variables neutral", () => {
    expect(captureToken("variable")).toBe("plain"); expect(captureToken("operator")).toBe("plain");
    expect(captureToken("variable.member")).toBe("property"); expect(captureToken("function.macro")).toBe("property");
    expect(captureToken("string.escape")).toBe("constructor"); expect(captureToken("punctuation.bracket")).toBe("special");
    expect(resolveCaptures(5, [{ start: 0, end: 5, name: "string", pattern: 0 }, { start: 1, end: 3, name: "string.escape", pattern: 1 }])).toEqual([
        { start: 0, end: 1, token: "string" }, { start: 1, end: 3, token: "constructor" }, { start: 3, end: 5, token: "string" },
    ]);
});
it("all syntax colors use live theme variables, with no hardcoded palette", async () => {
    const css = await readFile(new URL("./syntax.css", import.meta.url), "utf8");
    for (const token of ["keyword", "type", "string", "number", "comment", "func", "property", "constructor", "special"]) expect(css).toContain(`var(--theme-syn_${token}, var(--theme-fg, inherit))`);
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(/i);
});
