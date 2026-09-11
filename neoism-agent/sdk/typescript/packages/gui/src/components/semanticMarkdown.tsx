import "./semantic-markdown.css";

// Port of native agent_pane/view/markdown/inline_style.rs. Order is intentional:
// e.g. todo is workflow cyan, blocked is permission yellow, enabled is permission yellow.
export type SemanticColor = "accent" | "blue" | "cyan" | "magenta" | "yellow" | "syn_type" | "syn_string" | "green" | "red";
export interface SemanticStyle { color: SemanticColor; bold: boolean }
const agent_mode = new Set("agent agents subagent subagents build plan planning review debug assistant persona personas".split(" "));
const task_workflow = new Set("task tasks todo todos step steps workflow workflows session sessions message messages conversation history timeline prompt prompts command commands filename filenames attach attached fuzzy search copy paste clear undo redo resume restore revert continue jump navigate switch cycle toggle pin pinned slot slots sidebar panel parent child action actions change changes diff patch branch commit github issue issues pr prs".split(" "));
const tool_config = new Set("tool tools bash shell webfetch fetch edit write formatter formatters prettier gofmt ruff lsp mcp plugin plugins hook hooks config configuration setting settings theme themes json tui server servers api headless script scripts scripting notification notifications keybind keybinds leader palette shortcut shortcuts schema autocomplete format logs stderr stdout".split(" "));
const permission_policy = new Set("permission permissions allow deny ask approval approve blocked disable disabled enable enabled none auto manual public private share shared unshare protected external_directory doom_loop sensitive destructive".split(" "));
const model_context = new Set("model models provider providers llm context instructions rules temperature tokens input output clipboard image images pdf file files directory workspace codebase project zen terminal".split(" "));
const success = new Set("done success successful passed pass enabled ready complete completed".split(" "));
const warning = new Set("warning warn pending running todo note important".split(" "));
const error = new Set("error failed failure fail blocked bug fixme panic".split(" "));
const cleanTarget = (value: string) => value.replace(/^[,.:;)(\]\[}{<>`'"]+|[,.:;)(\]\[}{<>`'"]+$/g, "");
const extensions = new Set("rs ts tsx js jsx mjs cjs md mdx json jsonc toml yaml yml lua py go c h cpp hpp cxx java kt kts swift rb php sh bash zsh fish sql html htm css scss sass less vue svelte txt log csv tsv ini conf lock nix dockerfile".split(" "));
export function isNativeFileReference(value: string, inline = false): boolean {
    const target = cleanTarget(value);
    if (!target || /\s/.test(target)) return false;
    if (target.startsWith("file://")) return true;
    const base = target.split(":")[0];
    return /^(?:\/|\.\/|\.\.\/|~\/)/.test(base) || extensions.has(base.slice(base.lastIndexOf(".") + 1).toLowerCase()) && base.includes(".")
        || inline && base.includes("/") && !base.startsWith("//");
}
export function semanticTokenStyle(token: string): SemanticStyle | undefined {
    if (/^https?:\/\//.test(cleanTarget(token)) || isNativeFileReference(token)) return { color: "blue", bold: false };
    const clean = token.replace(/^[()\[\]{},.:;!?"']+|[()\[\]{},.:;!?"']+$/g, "");
    if (!clean) return undefined;
    if (/^[/@#]/.test(clean)) return { color: "cyan", bold: true };
    if (/^(?:\$|env:|file:|\{env:|\{file:)/.test(clean)) return { color: "magenta", bold: true };
    if (clean.startsWith("!") && clean.length > 1) return { color: "yellow", bold: false };
    if (clean.includes("+") && clean.split("+").every(p => /^(Ctrl|Cmd|Shift|Alt|Meta|Enter|Esc|Tab|Space)$/.test(p) || new TextEncoder().encode(p).length === 1)) return { color: "accent", bold: true };
    const lower = clean.toLowerCase();
    const groups: [Set<string>, SemanticColor][] = [[agent_mode, "magenta"], [task_workflow, "cyan"], [tool_config, "blue"], [permission_policy, "yellow"], [model_context, "syn_type"]];
    for (const [words, color] of groups) if (words.has(lower)) return { color, bold: true };
    if (/[A-Z]/.test(clean) && !/[a-z]/.test(clean) && (/[_-]/.test(clean) || clean.length > 1)) return { color: "syn_type", bold: true };
    for (const [words, color] of [[success, "green"], [warning, "yellow"], [error, "red"]] as const) if (words.has(lower)) return { color, bold: true };
    if (/::|\(\)|--|=/.test(clean) || /\.(rs|ts|tsx|js|jsx|json|toml|md|py|go|yaml|yml|nix|lua|sh|lock|log)$/.test(clean) || /\.(opencode|neoism)\/|~\/\.config\//.test(clean)) return { color: "syn_string", bold: false };
    return undefined;
}
// Structural HAST type avoids a direct dependency on transitive @types/hast.
interface Node { type: string; tagName?: string; value?: string; properties?: Record<string, unknown>; children?: Node[] }
/** Rehype plugin: use after code highlighting. Never touches code blocks, links or raw HTML.
 * Inline code follows native blue path / syn_string identifier, strong follows theme foreground.
 * File references are colored, not made executable links (host must own file navigation).
 */
export function rehypeSemanticMarkdown() {
    return (tree: Node) => {
        function walk(node: Node) {
            if (node.type === "element") {
                if (["pre", "a", "script", "style", "strong", "em", "del"].includes(node.tagName || "")) return;
                if (node.tagName === "code") {
                    const value = node.children?.map(child => child.value || "").join("") || "";
                    node.properties = { ...node.properties, className: [...(Array.isArray(node.properties?.className) ? node.properties.className : []), "neo-inline-code", isNativeFileReference(value, true) || /^https?:\/\//.test(value) ? "neo-semantic-blue" : "neo-semantic-syn_string"] };
                    return;
                }
            }
            if (!node.children) return;
            node.children = node.children.flatMap(child => {
                if (child.type !== "text" || !child.value) { walk(child); return [child]; }
                return child.value.split(/(\s+)/).filter(Boolean).map(value => {
                    const style = semanticTokenStyle(value);
                    return style ? { type: "element", tagName: "span", properties: { className: [`neo-semantic-${style.color}`, ...(style.bold ? ["neo-semantic-bold"] : [])] }, children: [{ type: "text", value }] } : { type: "text", value };
                });
            });
        }
        walk(tree);
    };
}
/** Native assistant.rs colors only agent blue; model/duration/rate stay muted. */
export function ResponseFooter({ value }: { value: string }) {
    const separator = value.indexOf(" · ");
    return <footer className="neo-response-footer" aria-label="Response metadata"><span className="neo-response-agent">{separator < 0 ? value : value.slice(0, separator)}</span>{separator < 0 ? "" : value.slice(separator)}</footer>;
}
