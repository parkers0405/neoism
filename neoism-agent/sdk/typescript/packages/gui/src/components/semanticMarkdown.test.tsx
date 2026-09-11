import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { isNativeFileReference, rehypeSemanticMarkdown, ResponseFooter, semanticTokenStyle } from "./semanticMarkdown";
describe("native prose semantic roles", () => {
    it("matches native precedence and category colors", () => {
        for (const [token, color] of [["Build", "magenta"], ["todo", "cyan"], ["API", "blue"], ["blocked", "yellow"], ["enabled", "yellow"], ["model", "syn_type"], ["NEOISM_LOG", "syn_type"], ["success", "green"], ["warning", "yellow"], ["panic", "red"], ["Ctrl+P", "accent"], ["$HOME", "magenta"], ["foo::bar", "syn_string"], ["src/main.rs:12", "blue"]]) expect(semanticTokenStyle(token)?.color).toBe(color);
        for (const token of ["ordinary", "42", "0.5", "a", "lowerCamelCase"]) expect(semanticTokenStyle(token)).toBeUndefined();
    });
    it("uses stricter file detection in prose than in backticks", () => {
        expect(isNativeFileReference("a/b")).toBe(false); expect(isNativeFileReference("a/b", true)).toBe(true);
        expect(isNativeFileReference("README.md")).toBe(true); expect(isNativeFileReference("word.txtish")).toBe(false);
    });
    it("preserves text, does not recolor fences/links/strong, colors inline code separately", () => {
        const html = renderToStaticMarkup(<ReactMarkdown rehypePlugins={[rehypeSemanticMarkdown]}>{"Build ordinary 42 **Build** [Build](https://example.com) `value` `a/b`\n\n```text\nBuild task error\n```"}</ReactMarkdown>);
        expect(html).toContain('class="neo-semantic-magenta neo-semantic-bold">Build</span> ordinary 42');
        expect(html).toContain("<strong>Build</strong>"); expect(html).toContain('<a href="https://example.com">Build</a>');
        expect(html).toContain('neo-inline-code neo-semantic-syn_string'); expect(html).toContain('neo-inline-code neo-semantic-blue');
        expect(html).toContain('class="language-text">Build task error\n</code>');
    });
    it("colors only the footer agent, leaving metadata calculations and text unchanged", () => {
        const html = renderToStaticMarkup(<ResponseFooter value="Build · GPT-5 · 2s · 30 tok/s" />);
        expect(html).toContain('<span class="neo-response-agent">Build</span> · GPT-5 · 2s · 30 tok/s');
    });
});
