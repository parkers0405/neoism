import { FileText, Pencil, Search, Terminal, GitBranch, Globe, Wrench } from "lucide-react";

/** Shared by tool parts and runtime completion envelopes; running tasks keep the square loop. */
export function ToolActivityIcon({ name, running = false }: { name: string; running?: boolean }) {
    if (running) return <svg className="tc-task-orbit" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">{[1, .7, .45, .2].map((opacity, i) => <circle className="tc-task-orbit-dot" key={i} cx="3" cy="3" r="1.4" fill="currentColor" opacity={opacity} style={{ animationDelay: `${-i * .12}s` }} />)}</svg>;
    const Icon = /task|subtask/.test(name) ? GitBranch : /web|fetch/.test(name) ? Globe
        : /read/.test(name) ? FileText : /edit|write|patch|replace/.test(name) ? Pencil
        : /grep|glob|search/.test(name) ? Search : /bash|shell|terminal/.test(name) ? Terminal : Wrench;
    return <Icon className="tc-category-icon" size={15} strokeWidth={1.6} aria-hidden="true" />;
}
