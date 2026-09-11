import { Language, Parser, Query } from "web-tree-sitter";
import { MAX_SOURCE, languageName, resolveCaptures, type Span } from "./tokens";

export interface SyntaxAssets { runtime: string; grammar: (language: string) => Promise<Uint8Array>; query: (language: string) => Promise<string> }
/** One engine per worker. Grammar/query promises are single-flight, independent of theme. */
export class SyntaxEngine {
    private initialized?: Promise<void>;
    private languages = new Map<string, Promise<{ language: Language; query: Query }>>();
    constructor(private assets: SyntaxAssets) {}
    async highlight(source: string, fence: string): Promise<Span[]> {
        const name = languageName(fence);
        if (!name || source.length > MAX_SOURCE || !source.length) return [];
        this.initialized ??= Parser.init({ locateFile: () => this.assets.runtime });
        await this.initialized;
        let loading = this.languages.get(name);
        if (!loading) {
            loading = Promise.all([this.assets.grammar(name), this.assets.query(name)]).then(async ([wasm, text]) => {
                const language = await Language.load(wasm);
                return { language, query: new Query(language, text) };
            });
            this.languages.set(name, loading);
        }
        const { language, query } = await loading;
        const parser = new Parser();
        try {
            parser.setLanguage(language);
            const deadline = performance.now() + 100;
            const tree = parser.parse(source, null, { progressCallback: () => performance.now() > deadline });
            if (!tree) return [];
            try {
                // web-tree-sitter indices refer to UTF-16, unlike Rust's UTF-8 ranges.
                const captures = query.captures(tree.rootNode, { matchLimit: 16384, timeoutMicros: 100_000 });
                if (query.didExceedMatchLimit()) return [];
                return resolveCaptures(source.length, captures.map(c => ({ start: c.node.startIndex, end: c.node.endIndex, name: c.name, pattern: c.patternIndex })));
            } finally { tree.delete(); }
        } finally { parser.delete(); }
    }
}
