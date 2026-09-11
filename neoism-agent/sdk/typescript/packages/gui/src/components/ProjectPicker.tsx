import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Check, ChevronDown, Plus, Search } from "lucide-react";
import type { NeoismClient } from "@neoism/sdk";
import { availableProjects, selectServerProject, projectName, rememberProject, savedProjects, uniqueProjects } from "../projectPicker";
import { isAbsoluteResourceDirectory } from "../resourceScope";
import { FolderPicker } from "./FolderPicker";
import "./project-picker.css";

export interface ProjectPickerProps {
    client?: NeoismClient;
    sessionId?: string;
    directory?: string;
    recentDirectories?: string[];
    /** Stable server URL + authenticated account identifier, never a bearer token.
     * Omit to disable local persistence (server-provided recents still work). */
    projectStorageScope?: string;
    onDirectoryChange?(directory: string): Promise<void> | void;
}
export function ProjectPicker({ client, sessionId, directory = "", recentDirectories = [], projectStorageScope, onDirectoryChange }: ProjectPickerProps) {
    const [open, setOpen] = useState(false);
    const [folders, setFolders] = useState(false);
    const [query, setQuery] = useState("");
    const [serverPath, setServerPath] = useState("");
    const [options, setOptions] = useState<string[]>([]);
    const [picks, setPicks] = useState<string[]>([]);
    const [pending, setPending] = useState(false);
    const [error, setError] = useState("");
    const root = useRef<HTMLDivElement>(null);
    const trigger = useRef<HTMLButtonElement>(null);
    const generation = useRef(0);
    useEffect(() => {
        generation.current++; setOpen(false); setFolders(false); setPending(false); setError(""); setOptions([]); setServerPath("");
        setPicks(savedProjects(projectStorageScope));
        return () => { generation.current++; };
    }, [client, sessionId, projectStorageScope]);
    useEffect(() => {
        if (!open || !client) return;
        let stale = false;
        availableProjects(client, sessionId).then(paths => { if (!stale) setOptions(paths); });
        return () => { stale = true; };
    }, [open, client, sessionId]);
    useEffect(() => {
        if (!open) return;
        const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
        document.addEventListener("pointerdown", outside);
        return () => document.removeEventListener("pointerdown", outside);
    }, [open]);
    useLayoutEffect(() => {
        if (!open) return;
        const place = () => {
            if (!trigger.current || !root.current) return;
            const rect = trigger.current.getBoundingClientRect();
            const viewport = window.visualViewport;
            const top = viewport?.offsetTop ?? 0;
            const bottom = top + (viewport?.height ?? window.innerHeight);
            const above = Math.max(0, rect.top - top - 16);
            const below = Math.max(0, bottom - rect.bottom - 16);
            const downward = above < 180 && below > above;
            root.current.style.setProperty("--project-menu-available", `${downward ? below : above}px`);
            root.current.dataset.menuPlacement = downward ? "below" : "above";
        };
        place();
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, { capture: true, passive: true });
        window.visualViewport?.addEventListener("resize", place);
        window.visualViewport?.addEventListener("scroll", place);
        return () => {
            window.removeEventListener("resize", place);
            window.removeEventListener("scroll", place, true);
            window.visualViewport?.removeEventListener("resize", place);
            window.visualViewport?.removeEventListener("scroll", place);
        };
    }, [open]);
    const commit = async (path: string) => {
        if (!client || !onDirectoryChange) throw new Error("Project selection is unavailable");
        const revision = generation.current;
        setPending(true); setError("");
        try {
            const selected = await selectServerProject(client, path);
            if (revision !== generation.current) return;
            await onDirectoryChange(selected);
            rememberProject(projectStorageScope, selected);
            if (revision === generation.current) { setPicks(p => uniqueProjects([selected, ...p]).slice(0, 20)); setOpen(false); }
        } catch (error) {
            if (revision === generation.current) throw error;
        } finally { if (revision === generation.current) setPending(false); }
    };
    const paths = uniqueProjects([directory, ...picks, ...recentDirectories, ...options]).filter(isAbsoluteResourceDirectory);
    return <div className="project-picker" ref={root} onKeyDown={e => {
        if (e.key === "Escape") { e.stopPropagation(); setOpen(false); trigger.current?.focus(); }
        if (open && ["ArrowDown", "ArrowUp"].includes(e.key)) {
            const buttons = Array.from(root.current!.querySelectorAll<HTMLButtonElement>(".project-menu button:not(:disabled)"));
            if (!buttons.length) return;
            e.preventDefault(); const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
            buttons[(index + (e.key === "ArrowDown" ? 1 : buttons.length - 1) + buttons.length) % buttons.length]?.focus();
        }
    }}>
        <button type="button" className="project-pill" ref={trigger} aria-label={`Choose project: ${projectName(directory)}`} title={directory || "Choose project"} aria-haspopup="dialog" aria-expanded={open} disabled={!client || !onDirectoryChange || pending} onClick={() => { setOpen(value => !value); setQuery(""); setError(""); }}>
            <span className="project-letter">P</span><span className="project-pill-name">{projectName(directory)}</span><ChevronDown size={13} />
        </button>
        {open && <div className="project-menu" role="dialog" aria-label="Projects">
            <label className="project-search"><Search size={15} /><input autoFocus aria-label="Search projects" placeholder="Search projects" value={query} onChange={e => setQuery(e.target.value)} /></label>
            <div className="project-menu-list">
                {paths.filter(path => path.toLowerCase().includes(query.toLowerCase())).map(path => <button type="button" className="project-menu-row" key={path} disabled={pending} title={path} aria-current={path === directory ? "true" : undefined} onClick={() => void commit(path).catch(err => setError(err instanceof Error ? err.message : "Could not open project"))}>
                    <span className="project-letter">P</span><span>{projectName(path)}<small>{path}</small></span>{path === directory && <Check size={15} />}
                </button>)}
                {!paths.filter(path => path.toLowerCase().includes(query.toLowerCase())).length && <p>No projects found</p>}
            </div>
            <form onSubmit={e => { e.preventDefault(); if (isAbsoluteResourceDirectory(serverPath)) void commit(serverPath).catch(err => setError(err instanceof Error ? err.message : "Could not open project")); }}>
                <label>Absolute server path<input aria-label="Absolute server path" placeholder="/path/to/project" value={serverPath} onChange={e => setServerPath(e.target.value)} /></label>
                <p className="muted">A path on the connected server, not this browser’s filesystem. Resource requests are checked by the server for access and path validity.</p>
                <button type="submit" disabled={pending || !isAbsoluteResourceDirectory(serverPath)}>Choose project</button>
            </form>
            {error && <p className="project-picker-error" role="alert">{error}</p>}
            <button type="button" className="project-add" disabled={pending} onClick={() => { setOpen(false); setFolders(true); }}><Plus size={16} />Add project</button>
        </div>}
        {folders && client && <FolderPicker client={client} directory={directory} recentDirectories={uniqueProjects([...picks, ...recentDirectories])} select={commit} close={() => { setFolders(false); trigger.current?.focus(); }} />}
    </div>;
}
