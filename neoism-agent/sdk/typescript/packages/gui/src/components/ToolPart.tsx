import { memo, useMemo, useState, type ReactNode } from "react";
import { Check, Circle, Clock, Code2, XCircle } from "lucide-react";
import { duration } from "./chatSupport";
import { cardData, clean, field, fileChanges, isFileTool, prettyPreview, taskActivityStatus, taskIdentity, toolName, type TaskChildStatus, type CardPart, type FileChange } from "./toolCardData";
import { stripTerminalControls } from "./runtimeMessages";
import { isTodoTool } from "../todoHelpers";
import { EditDiagnostics } from "./EditDiagnostics";
import { ToolTreePreview as TreePreview } from "./ToolTreePreview";
import "./tool-cards.css";

export interface ToolPartProps {
    part: CardPart;
    /** Current correlated runtime branch, never inferred from unrelated session activity. */
    childStatus?: TaskChildStatus;
    onOpenSession?: (sessionId: string) => void;
    /** Supply only when backed by the SDK task stop operation. IDs are task IDs, not session IDs. */
    onStopTask?: (taskId: string) => Promise<unknown>;
}
/** Only mounted beneath an expanded tool header. Reveal remains bounded and inline. */
function ValueView({ value }: { value: unknown }) {
    const [limit, setLimit] = useState(8000);
    const raw = useMemo(() => {
        if (typeof value === "string") return value;
        try { return JSON.stringify(value, null, 2) ?? ""; }
        catch { return prettyPreview(value, 8000); }
    }, [value]);
    const plain = useMemo(() => stripTerminalControls(raw), [raw]);
    const visible = plain.slice(0, limit);
    const more = plain.length > limit;
    return <div className="tc-value"><pre onScroll={event => {
        const node = event.currentTarget;
        if (more && node.scrollHeight - node.scrollTop - node.clientHeight < 80) setLimit(value => Math.min(plain.length, value + 8000));
    }}><code>{visible || "No content"}</code></pre><div className="tc-actions">
        {more && <button type="button" onClick={() => setLimit(n => n + 8000)}>Show more</button>}
    </div></div>;
}
function Status({ status, compact = false }: { status: string; compact?: boolean }) {
    const Icon = compact && status !== "error" ? Circle : status === "completed" ? Check : status === "error" ? XCircle : status === "running" ? Clock : Circle;
    return <span className={`tc-status tc-${status}`} aria-hidden={compact || undefined} title={status}>{compact && status === "completed" ? <span className="tc-status-dot" /> : compact && status === "running" ? <span className="tc-running-dots" aria-hidden="true">•••</span> : <Icon size={13} aria-hidden="true" />}{!compact && status}</span>;
}
function toolPreview(part: CardPart, shown = ""): string {
    const { input, state, status, error } = cardData(part);
    if (error) return clean(error.split("\n").map(line => line.trim()).find(Boolean) || "Tool failed", 180);
    const name = toolName(part);
    const query = Array.isArray(input.pattern) ? input.pattern.filter(value => typeof value === "string").join(", ") : field(input, "pattern", "query");
    if (/grep|search/.test(name) && query && query !== shown) return `Grep ${clean(query, 180)}`;
    const description = field(input, "description");
    if (description && description !== shown) return clean(description, 180);
    if (typeof state.output === "string" && state.output.trim()) {
        let text = stripTerminalControls(state.output.slice(0, 8000));
        if (/read/.test(name) && /^\s*<path>/.test(text)) {
            text = /<content>\s*([\s\S]*?)(?:<\/content>|$)/.exec(text)?.[1] || "";
            text = text.replace(/^\s*\d+:\s?/gm, "");
        }
        return clean(text.split("\n").map(line => line.trim()).filter(Boolean).slice(0, 2).join(" "), 180);
    }
    return status === "pending" || status === "running" ? "Waiting for output…" : "Output ready";
}
function Frame({ part, title, preview, compact, children }: { part: CardPart; title: ReactNode; preview?: string; compact?: (toggle: () => void) => ReactNode; children: ReactNode }) {
    const { state, status, error } = cardData(part);
    const [expanded, setExpanded] = useState(false);
    const elapsed = duration(state.time);
    const toggle = () => setExpanded(value => !value);
    return <section className={`tc-card tc-edit${expanded ? " tc-expanded" : ""}`} data-tool-status={status} aria-label={`${part.type === "tool" ? clean(part.tool) : "Task"}: ${status}`}>
        <button type="button" className="tc-header tc-tool-toggle" aria-expanded={expanded} onClick={toggle}>
            <Status status={status} compact /><span className="tc-title">{title} <span className="tc-state-word">{status}</span></span>{elapsed && <time className="tc-hover-time">{elapsed}</time>}
        </button>
        {!expanded && (compact && !error ? compact(toggle) : <TreePreview text={error ? toolPreview(part) : preview || toolPreview(part)} toggle={toggle} />)}
        {expanded && <div className="tc-tree-body">
            {error && <div className="tc-error"><ValueView value={state.error || error} /></div>}
            {children}
        </div>}
    </section>;
}
export function TaskToolCard({ part, childStatus, onOpenSession, onStopTask }: ToolPartProps) {
    const { status, state, error } = cardData(part), identity = taskIdentity(part);
    const [expanded, setExpanded] = useState(false), [stopping, setStopping] = useState(false), [stopError, setStopError] = useState("");
    const activity = taskActivityStatus(part, childStatus), active = activity === "pending" || activity === "running";
    const elapsed = duration(state.time);
    const output = state.output ?? (part.type === "subtask" ? part.prompt : undefined);
    return <section className={`tc-card tc-task${expanded ? " tc-expanded" : ""}`} data-tool-status={status} data-task-status={activity}>
        <div className="tc-task-row">
            <button type="button" className="tc-header tc-tool-toggle" aria-expanded={expanded} onClick={() => setExpanded(v => !v)}>
                <span className={`tc-task-indicator tc-${activity}`} aria-label={`Subagent ${activity}`}>
                    {active ? <svg className="tc-task-orbit" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">{[1, .7, .45, .2].map((opacity, i) => <circle className="tc-task-orbit-dot" key={i} cx="3" cy="3" r="1.4" fill="currentColor" opacity={opacity} style={{ animationDelay: `${-i * .12}s` }} />)}</svg> : activity === "completed" ? <Check size={15} aria-hidden="true" /> : activity === "error" ? <XCircle size={15} aria-hidden="true" /> : <Circle size={13} aria-hidden="true" />}
                </span>
                <span className="tc-title">Task({identity.description || cardData(part).title || "Delegated task"})</span>
                {elapsed && <time className="tc-hover-time">{elapsed}</time>}
            </button>
            {identity.sessionId && onOpenSession && <button type="button" className="tc-open-session" onClick={event => { event.stopPropagation(); onOpenSession(identity.sessionId); }}>Open session ↗</button>}
            {identity.taskId && onStopTask && active && <button type="button" disabled={stopping} onClick={async event => { event.stopPropagation(); setStopping(true); setStopError(""); try { await onStopTask(identity.taskId); } catch (e) { setStopError(clean(e instanceof Error ? e.message : "Unable to stop task")); } finally { setStopping(false); } }}>{stopping ? "Stopping…" : "Stop task"}</button>}
        </div>
        {active && !expanded && <p className="tc-waiting"><span aria-hidden="true">╰─</span> Waiting for task output…</p>}
        {!expanded && (error || activity === "error") && <TreePreview text={clean(error.split("\n")[0] || "Subagent failed.", 180)} toggle={() => setExpanded(true)} />}
        {stopError && <p className="tc-error" role="status">{stopError}</p>}
        {expanded && <div className="tc-tree-body">
            {error || activity === "error" ? <div className="tc-error"><ValueView value={state.error || error || "Subagent failed."} /></div> : active ? <p className="tc-summary">Waiting for task output…</p> : output !== undefined && output !== "" ? <ValueView value={output} /> : null}
        </div>}
    </section>;
}
function CompactDiff({ changes, toggle }: { changes: FileChange[]; toggle(): void }) {
    const change = changes.find(file => file.rows.length > 0) || changes[0];
    return <div className="tc-tree-body"><button type="button" tabIndex={-1} className="tc-compact-diff" aria-label="Expand full diff" onClick={toggle}>
        {(changes.length > 1 || change.moveTo) && <span className="tc-compact-file">{clean(change.path, 400)}{change.moveTo && ` → ${clean(change.moveTo, 400)}`}</span>}
        {change.rows.slice(0, 6).map((row, index) => <span className={`tc-diff-row tc-line-${row.kind}`} key={index}><span aria-hidden="true">{row.kind === "add" ? "+" : row.kind === "remove" ? "−" : " "}</span><code>{row.text}</code></span>)}
    </button></div>;
}
function Diff({ change, pending = false }: { change: FileChange; pending?: boolean }) {
    const [top, setTop] = useState(0);
    const [copyStatus, setCopyStatus] = useState("");
    const length = change.rows.length;
    const start = Math.max(0, Math.min(Math.floor(top / 18) - 4, length - 24));
    const rows = change.rows.slice(start, start + 34);
    const columns = useMemo(() => change.rows.reduce((width, row) => Math.max(width, row.text.length), 1), [change.rows]);
    return <div className="tc-patch-block">
        <div className="tc-patch-toolbar"><span>diff</span>
            {typeof navigator !== "undefined" && navigator.clipboard && <button type="button" onClick={async () => {
                const text = change.patch || change.rows.map(row => `${row.kind === "add" ? "+" : row.kind === "remove" ? "-" : row.kind === "context" ? " " : ""}${row.text}`).join("\n");
                try { await navigator.clipboard.writeText(stripTerminalControls(text)); setCopyStatus("Copied"); } catch { setCopyStatus("Clipboard unavailable"); }
            }}>{change.truncated ? "Copy preview" : "Copy diff"}</button>}
            {copyStatus && <span role="status">{copyStatus}</span>}
        </div>
        <div className="tc-diff" tabIndex={0} role="region" aria-label={`Diff for ${clean(change.path)}`} style={{height: Math.max(36, Math.min(400, length * 18))}} onScroll={event => setTop(event.currentTarget.scrollTop)}>
            {length > 0 && <div className="tc-diff-content" style={{height:length * 18,paddingTop:start * 18,boxSizing:"border-box",minWidth:`max(100%, calc(${columns}ch + 48px))`}}>
                {rows.map((row, index) => <div className={`tc-diff-row tc-line-${row.kind}`} data-diff-index={start + index} key={start + index}><span aria-hidden="true">{row.kind === "add" ? "+" : row.kind === "remove" ? "−" : " "}</span><code>{row.text}</code></div>)}
            </div>}
            {!length && (change.action === "delete" || pending) && <p className="tc-diff-note">{change.action === "delete" ? "File deletion requested." : "Pending edit…"}</p>}
        </div>
        {change.truncated && <p className="tc-diff-note">Showing a shortened diff preview.</p>}
    </div>;
}
export function FileEditCard({ part }: ToolPartProps) {
    const changes = useMemo(() => fileChanges(part), [part]);
    const { input, status } = cardData(part);
    const name = toolName(part);
    const verb = name === "apply_patch" ? "ApplyPatch" : name === "write" ? "Write" : "Edit";
    const target = changes.length === 1 ? changes[0].path : changes.length > 1 ? `${changes.length} files` : field(input, "filePath", "path", "file_path");
    const sample = changes[0]?.rows.find(row => row.kind === "add") || changes[0]?.rows.find(row => row.kind === "remove") || changes[0]?.rows.find(row => row.kind === "context");
    const preview = sample ? clean(`${sample.kind === "add" ? "+ " : sample.kind === "remove" ? "− " : ""}${sample.text}`, 180)
        : status === "pending" || status === "running" ? "Pending edit…" : toolPreview(part, target);
    return <>
        <Frame part={part} title={`${verb}${target ? `(${clean(target, 480)})` : ""}`} preview={preview}
            compact={changes.some(change => change.rows.length > 0) ? toggle => <CompactDiff changes={changes} toggle={toggle} /> : undefined}>
            {changes.length ? changes.map((change, i) => <div className="tc-file" key={`${change.path}:${i}`}><div className="tc-file-header"><Code2 size={14} aria-hidden="true" /><code>{clean(change.path, 1000)}{change.moveTo && <> → {clean(change.moveTo, 1000)}</>}</code><span>{clean(change.action)}{change.source === "input" ? " · proposed" : ""}</span>
                {!change.truncated && !change.rows.some(r => r.partial) && change.rows.some(r => r.kind === "add" || r.kind === "remove") && <span className="tc-counts"><b className="tc-added">+{change.rows.filter(r => r.kind === "add").length}</b><b className="tc-removed">−{change.rows.filter(r => r.kind === "remove").length}</b></span>}
            </div><Diff change={change} pending={status === "pending" || status === "running"} /></div>) : <div className="tc-summary">
                {(status === "pending" || status === "running") && <span>Pending edit…</span>}
                {name !== "apply_patch" && typeof input.content === "string" && <ValueView value={input.content} />}
            </div>}
        </Frame>
        <EditDiagnostics part={part} />
    </>;
}
function CommonToolCard({ part }: ToolPartProps) {
    const name = toolName(part), { input, state, title, status, error } = cardData(part);
    const [expanded, setExpanded] = useState(false);
    const verb = /bash|shell|terminal/.test(name) ? "Bash" : /grep/.test(name) ? "Grep" : /search/.test(name) ? "Search" : /glob/.test(name) ? "Glob" : /list/.test(name) ? "List" : /read/.test(name) ? "Read" : /fetch|web/.test(name) ? "Fetch" : title;
    const summary = field(input, "command", "cmd", "filePath", "path", "pattern", "query", "url", "description") || (title !== verb && title !== (part.type === "tool" ? part.tool : "") ? title : "");
    const elapsed = duration(state.time);
    return <section className={`tc-card tc-ordinary${expanded ? " tc-expanded" : ""}`} data-tool-status={status}>
        <button type="button" className="tc-header tc-tool-toggle" aria-expanded={expanded} onClick={() => setExpanded(v => !v)}>
            <Status status={status} compact />
            <span className="tc-call"><strong className="tc-verb">{verb}</strong>{summary && <span className="tc-argument">({clean(summary, 480)}{summary.length > 480 ? "…" : ""})</span>} <span className="tc-state-word">{status}</span></span>
            {elapsed && <time className="tc-hover-time">{elapsed}</time>}
        </button>
        {!expanded && <TreePreview text={toolPreview(part, summary)} toggle={() => setExpanded(true)} />}
        {expanded && <div className="tc-tree-body">
            {error ? <div className="tc-error"><ValueView value={state.error || error} /></div> :
                state.output !== undefined && state.output !== "" ? <ValueView value={state.output} /> :
                <p className="tc-summary">{status === "pending" || status === "running" ? "Waiting for tool output…" : "No output returned."}</p>}
            {summary.length > 480 && <ValueView value={summary} />}
        </div>}
    </section>;
}
/** PartView integration: case "tool": case "subtask": return <ToolPart part={part} ...callbacks />. */
export const ToolPart = memo(function ToolPart(props: ToolPartProps) {
    if (isTodoTool(props.part) && props.part.type === "tool" && props.part.state.status === "completed") return null;
    const name = toolName(props.part);
    return name === "task" || name === "task_result" || name === "subtask" ? <TaskToolCard {...props} /> : isFileTool(name) ? <FileEditCard {...props} /> : <CommonToolCard {...props} />;
});
