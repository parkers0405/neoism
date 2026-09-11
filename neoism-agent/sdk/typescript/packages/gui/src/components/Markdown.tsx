import { Children, isValidElement, useState, type ComponentProps, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { SyntaxCode } from "../syntax/SyntaxCode";
import { rehypeSemanticMarkdown } from "./semanticMarkdown";
import { MarkdownTodoInput } from "./TodoPanel";
import "./message-presentation.css";

/** Read text without flattening the rendered syntax-highlight nodes or trimming whitespace. */
export function codeText(node: ReactNode): string {
    if (typeof node === "string" || typeof node === "number") return String(node);
    if (isValidElement<{ children?: ReactNode }>(node)) return codeText(node.props.children);
    return Children.toArray(node).map(child => isValidElement<{ children?: ReactNode }>(child) ? codeText(child.props.children) : typeof child === "string" || typeof child === "number" ? String(child) : "").join("");
}
export function codeLanguage(children: ReactNode): string {
    const code = Children.toArray(children).find(child => isValidElement(child) && child.type === "code");
    if (!isValidElement<{ className?: string }>(code)) return "text";
    return /(?:^|\s)language-([^\s]+)/.exec(code.props.className ?? "")?.[1] || "text";
}
export async function copyCode(value: string, write: (text: string) => Promise<void>): Promise<string | undefined> {
    try { await write(value); return undefined; }
    catch { return "Could not copy code. Check clipboard permissions and try again."; }
}
function CodeBlock({ children, node: _node, ...props }: ComponentProps<"pre"> & { node?: unknown }) {
    const [copied, setCopied] = useState(false), [error, setError] = useState("");
    return <div className="neo-code-card">
        <div className="neo-code-header"><span>{codeLanguage(children)}</span>
            <button type="button" aria-label={copied ? "Copied" : "Copy code"} onClick={async () => {
                setError(""); setCopied(false);
                const failure = await copyCode(codeText(children), text => navigator.clipboard.writeText(text));
                if (failure) setError(failure); else setCopied(true);
            }}>{copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}</button>
        </div>
        <pre {...props}><SyntaxCode source={codeText(children)} language={codeLanguage(children)} /></pre>
        {copied && <span className="neo-copy-status" role="status">Code copied to clipboard</span>}
        {error && <small className="neo-copy-error" role="alert">{error}</small>}
    </div>;
}
export function Markdown({ text }: { text: string }) {
    return <div className="markdown">
        <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSemanticMarkdown]} skipHtml components={{
            pre: CodeBlock,
            input: MarkdownTodoInput,
            table: ({ children }) => <div className="markdown-table-scroll" tabIndex={0} role="region" aria-label="Scrollable table"><table>{children}</table></div>,
            a: ({ children, node: _node, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener">{children}</a>,
            img: ({ alt }) => <span className="muted">[Image: {alt || "external image"}]</span>,
        }}>{text}</ReactMarkdown>
    </div>;
}
