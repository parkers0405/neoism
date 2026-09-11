import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ArrowLeft, Check, Folder, Search, X } from "lucide-react";
import type { NeoismClient } from "@neoism/sdk";
import { listServerFolders, projectName, rememberProject, savedProjects, uniqueProjects, type DirectoryListing } from "../projectPicker";
import { SkeletonRows } from "./Skeleton";
import "./project-picker.css";

export interface FolderPickerProps {
    client: NeoismClient;
    directory?: string;
    recentDirectories?: string[];
    projectStorageScope?: string;
    select(directory: string): Promise<void> | void;
    close(): void;
}
export function FolderPicker(props: FolderPickerProps) {
    const [scope, setScope] = useState({ client: props.client, directory: props.directory, key: 0 });
    if (scope.client !== props.client || scope.directory !== props.directory) {
        setScope({ client: props.client, directory: props.directory, key: scope.key + 1 });
    }
    return <ScopedFolderPicker key={scope.key} {...props} />;
}
function ScopedFolderPicker({ client, directory, recentDirectories = [], projectStorageScope, select, close }: FolderPickerProps) {
    const recents = uniqueProjects([...savedProjects(projectStorageScope), ...recentDirectories]);
    const dialog = useRef<HTMLDialogElement>(null);
    const [location, setLocation] = useState<string | undefined>(directory || undefined);
    const [refresh, setRefresh] = useState(0);
    const [listing, setListing] = useState<DirectoryListing>();
    const [selected, setSelected] = useState("");
    const [search, setSearch] = useState("");
    const [pathInput, setPathInput] = useState("");
    const [history, setHistory] = useState<string[]>([]);
    const [loading, setLoading] = useState(true);
    const [pending, setPending] = useState(false);
    const [error, setError] = useState("");
    const validation = useRef(0);
    const live = useRef(true);
    useEffect(() => { live.current = true; const el = dialog.current!; el.showModal(); return () => { live.current = false; validation.current++; el.close(); }; }, []);
    useEffect(() => { validation.current++; setLocation(directory || undefined); setHistory([]); setSearch(""); setPending(false); }, [client, directory]);
    useEffect(() => {
        const abort = new AbortController();
        validation.current++; setLoading(true); setSelected(""); setError(""); setListing(undefined);
        listServerFolders(client, location, abort.signal).then(result => {
            if (abort.signal.aborted) return;
            setListing(result); setSelected(result.path); setPathInput(result.path); setLoading(false);
        }).catch(err => { if (!abort.signal.aborted) { setError(String(err instanceof Error ? err.message : err)); setLoading(false); } });
        return () => abort.abort();
    }, [client, location, refresh]);
    const navigate = (path: string, back = false) => {
        if (path === listing?.path) { setSelected(path); setSearch(""); return; }
        validation.current++; setSelected(""); setSearch(""); setLoading(true); setListing(undefined); setPathInput(path);
        if (!back && listing && path !== listing.path) setHistory(h => [...h, listing.path]);
        if (path === location) setRefresh(n => n + 1);
        setLocation(path);
    };
    const choose = async (path: string, trusted = false) => {
        const revision = ++validation.current; setError(""); setSelected(trusted ? path : "");
        if (trusted) return;
        try { const result = await listServerFolders(client, path); if (live.current && revision === validation.current) setSelected(result.path); }
        catch (err) { if (live.current && revision === validation.current) setError(err instanceof Error ? err.message : "Folder is unavailable"); }
    };
    const confirm = async () => {
        if (!selected || pending || loading) return;
        const revision = validation.current;
        setPending(true); setError("");
        try {
            // Revalidate even a recent path immediately before committing; deleted
            // folders and changed symlinks must never silently become defaults.
            const result = await listServerFolders(client, selected);
            if (!live.current || revision !== validation.current) return;
            await select(result.path);
            rememberProject(projectStorageScope, result.path);
            if (live.current) close();
        } catch (err) { if (live.current) setError(err instanceof Error ? err.message : "Could not open project"); }
        finally { if (live.current) setPending(false); }
    };
    const matches = (path: string) => path.toLowerCase().includes(search.toLowerCase());
    const row = (path: string, name: string, trusted: boolean) => <button type="button" className="project-folder-row" key={path} role="option" aria-selected={selected === path} disabled={pending || loading}
        onClick={() => void choose(path, trusted)} onDoubleClick={() => navigate(path)} onKeyDown={e => { if (e.key === "ArrowRight") { e.preventDefault(); navigate(path); } }} title={path}>
        <Folder size={17} /><span>{name}<small>{path}</small></span>{selected === path && <Check size={16} />}
    </button>;
    const content = <dialog className="project-folder-dialog" ref={dialog} aria-labelledby="open-project-title" onCancel={e => { e.preventDefault(); if (!pending) close(); }} onClick={e => { if (e.target === e.currentTarget && !pending) close(); }}>
        <section className="project-folder-content">
            <header><h2 id="open-project-title">Open project</h2><button type="button" aria-label="Close Open project" disabled={pending} onClick={close}><X size={18} /></button></header>
            <div className="project-folder-navigation">
                {(history.length > 0 || listing?.parent) && <button type="button" aria-label="Back" disabled={pending || loading} onClick={() => {
                    const target = history.at(-1) || listing?.parent;
                    if (target) { setHistory(h => h.slice(0, -1)); navigate(target, true); }
                }}><ArrowLeft size={16} /></button>}
                <form onSubmit={e => { e.preventDefault(); if (!pending && pathInput.trim()) navigate(pathInput.trim()); }}>
                    <input aria-label="Server folder path" value={pathInput} disabled={pending} placeholder="Server folder path" onChange={e => { validation.current++; setSelected(""); setPathInput(e.target.value); }} />
                </form>
            </div>
            <label className="project-search"><Search size={16} /><input autoFocus aria-label="Search folders" placeholder="Search folders" value={search} onChange={e => setSearch(e.target.value)} /></label>
            <div className="project-folder-scroll" aria-busy={loading}>
                {loading && <SkeletonRows kind="folder" count={6} header />}
                {!loading && <>
                    {!!recents.length && <div role="listbox" aria-label="Recent projects"><h3>Recent projects</h3>{recents.filter(matches).map(path => row(path, projectName(path), false))}</div>}
                    {listing && <div role="listbox" aria-label="Open project directories"><h3>Folders in {projectName(listing.path)}</h3>
                        {listing.entries.filter(entry => matches(entry.name)).map(entry => row(entry.path, entry.name, true))}
                        {!listing.entries.filter(entry => matches(entry.name)).length && <p>No folders found</p>}
                    </div>}
                </>}
            </div>
            {error && <p className="project-picker-error" role="alert">{error}</p>}
            <footer><span title={selected}>{selected ? projectName(selected) : "Select a folder"}</span><button type="button" className="project-folder-confirm" disabled={!selected || loading || pending} onClick={() => void confirm()}>{pending ? "Opening…" : "Select folder"}</button></footer>
        </section>
    </dialog>;
    return typeof document === "undefined" ? content : createPortal(content, document.body);
}
