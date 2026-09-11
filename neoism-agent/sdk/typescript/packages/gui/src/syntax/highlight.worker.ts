import runtime from "web-tree-sitter/tree-sitter.wasm?url";
import { SyntaxEngine } from "./engine";
import type { Span } from "./tokens";
const base = `${import.meta.env.BASE_URL}syntax/`;
async function asset(path: string) {
    const response = await fetch(base + path);
    if (!response.ok) throw new Error(`Syntax asset unavailable: ${path}`);
    return response;
}
const engine = new SyntaxEngine({ runtime, grammar: async name => new Uint8Array(await (await asset(`${name}.wasm`)).arrayBuffer()), query: async name => (await asset(`${name}.scm.txt`)).text() });
const cache = new Map<string, Span[]>();
let cachedUnits = 0;
self.onmessage = async ({ data }: MessageEvent<{ id: number; source: string; language: string }>) => {
    const { id, source, language } = data;
    const key = language + "\0" + source;
    try {
        let spans = cache.get(key);
        if (!spans) {
            spans = await engine.highlight(source, language);
            if (!cache.has(key)) { cache.set(key, spans); cachedUnits += key.length; }
            while (cache.size > 128 || cachedUnits > 512 * 1024) {
                const oldest = cache.keys().next().value!; cache.delete(oldest); cachedUnits -= oldest.length;
            }
        }
        self.postMessage({ id, spans });
    } catch { self.postMessage({ id, spans: [] }); }
};
