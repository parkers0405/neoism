import { useEffect, useLayoutEffect, useId, useRef, useState } from "react";
import { ArrowUp, Square, ChevronDown, Plus, X } from "lucide-react";
import type { SlashCommand } from "../types";
import { filterCommands, moveSelection } from "../commands";
import { ProjectPicker, type ProjectPickerProps } from "./ProjectPicker";
import "./composer-native.css";
import { NativeFooterActivity } from "./NativeFooterActivity";
import { AttachmentPreview } from "./AttachmentPreview";
export interface Choice { id: string; label: string; description?: string; section?: string; badge?: string }
export interface ComposerProps extends ProjectPickerProps {
    busy: boolean;
    commands: SlashCommand[];
    send(text: string, files?: File[]): Promise<void>;
    abort(): void;
    model: string;
    agent: string;
    thinking: string;
    openPicker(type: string): void;
    draftInsertion?: { text: string; revision: number };
    tabKey?: string;
    draft?: string;
    onDraftChange?(text: string): void;
    files?: File[];
    onFilesChange?(files: File[]): void;
    directory?: string;
    onCycleAgent?(): void;
    showHints?: boolean;
    /** App hosts the footer outside its capped input scroller. */
    showFooter?: boolean;
}
type TabDraft = { text: string; files: File[]; pending: boolean; error: string; revision?: number };
const empty = (): TabDraft => ({ text: "", files: [], pending: false, error: "" });
const sizeLabel = (bytes: number) => bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1048576).toFixed(1)} MB`;

export function Composer({ busy, commands, send, abort, model, agent, thinking, openPicker,
    draftInsertion, tabKey = "legacy", draft, onDraftChange, files: controlledFiles,
    onFilesChange, directory, client, sessionId, recentDirectories, projectStorageScope, onDirectoryChange, onCycleAgent, showHints = true, showFooter = true }: ComposerProps) {
    // Unkeyed hosts retain local drafts/attachments across tab switches. Keyed hosts
    // must supply controlled files as well as controlled drafts to retain both.
    const tabs = useRef(new Map<string, TabDraft>());
    const [, redraw] = useState(0);
    if (!tabs.current.has(tabKey)) tabs.current.set(tabKey, empty());
    const state = tabs.current.get(tabKey)!;
    const text = draft ?? state.text;
    const files = controlledFiles ?? state.files;
    state.text = text;
    state.files = files;
    const currentKey = useRef(tabKey);
    currentKey.current = tabKey;
    const alive = useRef(true);
    useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
    const update = () => { if (alive.current) redraw(n => n + 1); };
    const setText = (value: string) => { state.text = value; onDraftChange?.(value); update(); };
    const setFiles = (value: File[]) => { state.files = value; onFilesChange?.(value); update(); };
    const [index, setIndex] = useState(0);
    const [dismissed, setDismissed] = useState(false);
    const ref = useRef<HTMLTextAreaElement>(null);
    const fileInput = useRef<HTMLInputElement>(null);
    const menu = useRef<HTMLDivElement>(null);
    const menuId = useId();
    useEffect(() => {
        if (!draftInsertion || state.revision === draftInsertion.revision) return;
        state.revision = draftInsertion.revision;
        const old = state.text.startsWith("/skill") ? "" : state.text;
        setText(`${old}${old && !/\s$/.test(old) ? " " : ""}${draftInsertion.text}`);
        ref.current?.focus();
    }, [draftInsertion, tabKey]);
    const filtered = filterCommands(commands, text);
    const visible = text.startsWith("/") && !text.includes(" ") && !dismissed && filtered.length > 0;
    const selected = Math.min(index, Math.max(0, filtered.length - 1));
    useLayoutEffect(() => {
        setIndex(0); setDismissed(false);
        if (ref.current && !globalThis.CSS?.supports?.("field-sizing", "content")) {
            ref.current.style.height = "22px";
            ref.current.style.height = Math.min(ref.current.scrollHeight, 110) + "px";
        }
    }, [text, tabKey]);
    useEffect(() => { menu.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" }); }, [selected]);
    const submit = async (value = text) => {
        if ((!value.trim() && !files.length) || state.pending) return;
        const originalText = text;
        const originalFiles = [...files];
        state.pending = true; state.error = ""; update();
        try {
            await send(value, originalFiles.length ? originalFiles : undefined);
            // Never use a new tab's setter or erase edits made while uploading.
            if (alive.current && currentKey.current === tabKey) {
                if (state.text === originalText) setText("");
                setFiles(state.files.filter(file => !originalFiles.includes(file)));
            } else if (alive.current) {
                if (state.text === originalText) state.text = "";
                state.files = state.files.filter(file => !originalFiles.includes(file));
            }
        } catch (error) {
            state.error = error instanceof Error ? error.message : "Message could not be sent. Your draft and attachments are retained; try again.";
        } finally {
            state.pending = false; update();
            if (alive.current && currentKey.current === tabKey && !ref.current?.closest(".composer-anchor")?.querySelector(".composer-panel")) ref.current?.focus();
        }
    };
    return <div className="composer-wrap native-composer">
        {visible && <div className="slash-picker" role="listbox" aria-label="Commands" id={menuId} ref={menu}>
            <header>Commands <small>↑ ↓ / Tab navigate · Enter run</small></header>
            {filtered.map((c, i) => <button type="button" key={c.name} id={`${menuId}-${i}`} role="option" aria-selected={selected === i}
                onMouseEnter={() => setIndex(i)} onMouseDown={e => e.preventDefault()}
                onClick={() => void submit("/" + c.name.replace(/^\//, ""))}>
                <strong>/{c.name.replace(/^\//, "")}</strong><span>{c.description}</span>
            </button>)}
        </div>}
        <div className="native-composer-island">
            <div className="composer native-composer-surface">
                {!!files.length && <ul className="native-composer-attachments" aria-label="Selected attachments">
                    {files.map((file, i) => <li key={`${file.name}-${i}`} title={`${file.name} · ${sizeLabel(file.size)}`}>
                        <AttachmentPreview key={tabKey} file={file} />
                        <button type="button" aria-label={`Remove ${file.name}`} onClick={() => setFiles(files.filter((_, j) => i !== j))}><X size={12} /></button>
                    </li>)}
                </ul>}
                <textarea ref={ref} aria-label="Message Neoism" aria-controls={visible ? menuId : undefined} aria-expanded={visible}
                    aria-activedescendant={visible ? `${menuId}-${selected}` : undefined}
                    placeholder="Ask anything, or build something…" value={text} rows={1}
                    onChange={e => setText(e.target.value)} onKeyDown={e => {
                        if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
                        if (visible && ["ArrowDown", "ArrowUp", "Tab", "Enter", "Escape"].includes(e.key)) {
                            if (e.key === "Enter" && e.shiftKey) return;
                            e.preventDefault();
                            if (e.key === "Escape") setDismissed(true);
                            else if (["ArrowDown", "ArrowUp", "Tab"].includes(e.key)) setIndex(i => moveSelection(i, e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey) ? -1 : 1, filtered.length));
                            else void submit("/" + filtered[selected].name.replace(/^\//, ""));
                        } else if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void submit(); }
                        else if (e.key === "Tab" && !e.shiftKey && onCycleAgent) { e.preventDefault(); onCycleAgent(); }
                    }} />
                <div className="native-composer-input-band">
                    <input ref={fileInput} type="file" multiple hidden aria-label="Attach files" onChange={e => {
                        const added = Array.from(e.target.files ?? []);
                        setFiles([...files, ...added]); state.error = ""; e.target.value = "";
                    }} />
                    <button type="button" className="native-composer-add" title="Attach files" aria-label="Add attachments" onClick={() => fileInput.current?.click()}><Plus size={20} /></button>
                    {busy ? <button type="button" className="send native-composer-send" aria-label="Stop response" onClick={abort}><Square size={16} /></button>
                        : <button type="button" className="send native-composer-send" aria-label="Send message" disabled={(!text.trim() && !files.length) || state.pending} onClick={() => void submit()}><ArrowUp size={18} /></button>}
                </div>
            </div>
            <div className="native-composer-skirt">
                <div className="native-composer-chips">
                    <button type="button" data-agent={agent.toLowerCase()} className="native-composer-agent" onClick={() => openPicker("agent")} title={`Agent: ${agent}`} aria-label={`Choose agent: ${agent}`}><span>{agent}</span><ChevronDown size={12} /></button>
                    <button type="button" className="native-composer-model" onClick={() => openPicker("model")} title={`Model: ${model || "Select model"}`} aria-label={`Choose model: ${model || "Select model"}`}><span>{model || "Select model"}</span><ChevronDown size={12} /></button>
                    <button type="button" className="native-composer-effort" onClick={() => openPicker("thinking")} title={`Thinking: ${thinking || "Default"}`} aria-label={`Choose thinking: ${thinking || "Default"}`}><span>{thinking || "Thinking"}</span><ChevronDown size={12} /></button>
                    <NativeFooterActivity busy={busy && !!sessionId} />
                </div>
            </div>
        </div>
        {state.error && <p className="native-composer-error" role="alert">{state.error}</p>}
        {showFooter && <ComposerFooter busy={busy && !!sessionId} tabKey={tabKey} client={client} sessionId={sessionId} directory={directory} recentDirectories={recentDirectories} projectStorageScope={projectStorageScope} onDirectoryChange={onDirectoryChange} showHints={showHints} />}
    </div>;
}

export interface ComposerFooterProps extends ProjectPickerProps {
    tabKey?: string;
    showHints?: boolean;
    busy?: boolean;
    /** Home-only project selector; session-backed standalone composers hide it too. */
    showProject?: boolean;
}

export function ComposerFooter({ tabKey, client, sessionId, directory, recentDirectories, projectStorageScope, onDirectoryChange, showHints = true, busy = false, showProject = !sessionId }: ComposerFooterProps) {
    if (sessionId) return null;
    return (<div className="native-composer-footer">
            {showProject && <ProjectPicker key={tabKey} client={client} sessionId={sessionId} directory={directory} recentDirectories={recentDirectories} projectStorageScope={projectStorageScope} onDirectoryChange={onDirectoryChange} />}
            {showHints && <span className="native-composer-hints"><span><kbd>tab</kbd> agents</span><span><kbd>/</kbd> commands</span></span>}
        </div>);
}
