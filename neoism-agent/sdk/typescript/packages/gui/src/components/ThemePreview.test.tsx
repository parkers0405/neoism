import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ThemePreview } from "./ThemePreview";
import { themes } from "../generated/themes";

const expectedSample = `pub struct Theme {
    name: String,
}
// Live syntax preview
let theme = Theme::load("neoism");
if theme.contrast > 4.5 {
    render(&theme);
}`;

function nodes(node: any): any[] {
    if (!node || typeof node !== "object") return [];
    if (Array.isArray(node)) return node.flatMap(nodes);
    return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
    if (typeof node === "string") return node;
    if (Array.isArray(node)) return node.map(text).join("");
    return text(node?.props?.children ?? "");
}

describe("native Rust theme preview", () => {
    it.each(["pastelbeans", "github_light"])("uses %s candidate colors, not the current app tokens", id => {
        const theme = themes.find(t => t.id === id)!;
        const tree = ThemePreview({ theme });
        expect(tree.props.style).toEqual({ backgroundColor: theme.colors.bg, color: theme.colors.fg, borderColor: theme.colors.border });
        const spans = nodes(tree).filter(node => node.props?.["data-token"]);
        for (const node of spans) {
            expect(node.props.style.color).toBe(theme.colors[node.props["data-token"]]);
            expect(node.props.style.color).toMatch(/^#[\da-f]{6}$/i);
        }
        const tokenColor = (token: string) => spans.find(node => node.props["data-token"] === token).props.style.color;
        expect(tokenColor("syn_comment")).toBe(theme.colors.syn_comment);
        expect(tokenColor("syn_string")).toBe(theme.colors.syn_string);
        expect(tokenColor("syn_keyword")).toBe(theme.colors.syn_keyword);
        expect(tokenColor("syn_number")).toBe(theme.colors.syn_number);
        expect(text(nodes(tree).find(node => node.type === "code"))).toBe(expectedSample);
        const html = renderToStaticMarkup(<ThemePreview theme={theme} />);
        expect(html).toContain(`background-color:${theme.colors.bg}`);
        expect(html).toContain(`border-color:${theme.colors.border}`);
        expect(html).toContain("↑↓ preview · Enter apply");
    });
    it("renders the same exact sample with complete native colors for all 101 themes", () => {
        expect(themes).toHaveLength(101);
        for (const theme of themes) {
            const tree = ThemePreview({ theme });
            expect(text(nodes(tree).find(node => node.type === "code"))).toBe(expectedSample);
            for (const node of nodes(tree).filter(node => node.props?.["data-token"])) {
                expect(node.props.style.color, `${theme.id}: ${node.props["data-token"]}`).toMatch(/^#[\da-f]{6}$/i);
            }
        }
    });
    it("uses native split/card metrics and hides the preview by container width, not viewport", () => {
        const css = readFileSync(new URL("./theme-preview.css", import.meta.url), "utf8");
        for (const rule of ["min(300px, 45%)", "gap: 14px", "border-radius: 7px", "padding: 16px", "font-size: 12px", "line-height: 25px", '"Neoism JetBrains Mono"'])
            expect(css).toContain(rule);
        expect(css).toContain("@container theme-picker (max-width: 560px)");
        expect(css).toContain(".theme-preview,\n    .settings-theme-browser::before { display: none; }");
        expect(css).not.toContain("@media");
    });
});
