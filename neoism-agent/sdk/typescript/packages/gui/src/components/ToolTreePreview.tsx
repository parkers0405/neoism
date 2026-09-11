import "./tool-cards.css";

export function ToolTreePreview({ text, toggle }: { text: string; toggle(): void }) {
    return <button type="button" tabIndex={-1} className="tc-preview" onClick={toggle}>
        <span className="tc-preview-branch" aria-hidden="true">╰─</span>
        <span className="tc-preview-copy"><span>{text}</span><span className="tc-preview-hint">click to expand</span></span>
    </button>;
}
