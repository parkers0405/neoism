import { useState } from "react";
import type { Editor, SkillWrite } from "../management";
import { StructuredValueEditor } from "./StructuredValueEditor";

export function skillValidation(editor: Extract<Editor, { kind: "skills" }>): Record<string, string> {
    const errors: Record<string, string> = {};
    const bytes = (s: string) => new TextEncoder().encode(s).length;
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(editor.id)) errors.id = "Use a lowercase slug, such as code-review.";
    const d = editor.definition;
    if (!d.name?.trim()) errors.name = "Give your skill a name.";
    if (!d.content.trim()) errors.content = "Add instructions for your agent.";
    if (bytes(d.content) > 512 * 1024) errors.content = "Instructions must be 512 KiB or smaller.";
    const files = Object.entries(d.files ?? {});
    if (files.length > 32) errors.files = "A skill can contain at most 32 support files.";
    for (const [path, content] of files) {
        if (!validFilePath(path)) errors.files = "Use relative file paths without empty, dot or parent segments; SKILL.md is reserved.";
        if (bytes(content) > 256 * 1024) errors.files = `${path} exceeds 256 KiB.`;
    }
    // The backend limits decoded content bytes, not JSON transport escaping or metadata.
    const bundleBytes = bytes(d.content) + files.reduce((total, [, content]) => total + bytes(content), 0);
    if (bundleBytes > 1024 * 1024) errors.files = "The complete bundle must be 1 MiB or smaller.";
    return errors;
}
export function validFilePath(path: string): boolean {
    return !!path && !path.includes("\\") && !path.includes(":") && !/[\x00-\x1f]/.test(path) && path !== "SKILL.md" && path.split("/").every(p => !!p && p !== "." && p !== "..");
}
export function renameSupportFile(files: Record<string, string>, from: string, to: string): Record<string, string> {
    if (!validFilePath(to) || (to !== from && Object.hasOwn(files, to))) throw Error("Choose a unique relative file path. SKILL.md is reserved.");
    return Object.fromEntries(Object.entries(files).map(([path, content]) => [path === from ? to : path, content]));
}
function SupportFile({ path, content, disabled, rename, update, remove }: { path: string; content: string; disabled: boolean; rename(path: string): void; update(content: string): void; remove(): void }) {
    const [draft, setDraft] = useState(path);
    const [error, setError] = useState("");
    return <section className="support-file"><div className="resource-inline"><label className="form-field">File path<input disabled={disabled} value={draft} onChange={e => { setDraft(e.target.value); e.target.setCustomValidity(e.target.value !== path ? "Apply the file rename before saving." : ""); }} /></label><button type="button" disabled={disabled || draft === path} onClick={() => { try { rename(draft); setError(""); } catch (e) { setError(e instanceof Error ? e.message : "Invalid path"); } }}>Rename</button><button type="button" disabled={disabled} aria-label={`Remove ${path}`} onClick={remove}>Remove</button></div>{error && <p role="alert" className="error">{error}</p>}<label className="form-field">{path}<textarea className="resource-code" rows={6} readOnly={disabled} value={content} onChange={e => update(e.target.value)} /></label></section>;
}
export function SkillDefinitionEditor({ editor, change, errors = {} }: { editor: Extract<Editor, { kind: "skills" }>; change(editor: Editor): void; errors?: Record<string, string> }) {
    const d = editor.definition;
    const disabled = !!editor.readOnly;
    const [newPath, setNewPath] = useState("");
    const [fileError, setFileError] = useState("");
    const patch = (next: Partial<SkillWrite>) => change({ ...editor, definition: { ...d, ...next } });
    const field = (label: string, key: "name" | "description" | "content" | "version" | "license", rows?: number) => <label className="form-field">{label}{rows ? <textarea className={key === "content" ? "resource-code" : ""} rows={rows} readOnly={disabled} value={d[key] ?? ""} aria-invalid={!!errors[key]} onChange={e => patch({ [key]: e.target.value })} /> : <input readOnly={disabled} value={d[key] ?? ""} aria-invalid={!!errors[key]} onChange={e => patch({ [key]: e.target.value })} />}{errors[key] && <span role="alert" className="error">{errors[key]}</span>}</label>;
    return <div className="definition-form skill-definition">
        <section className="resource-fields">{field("Name", "name")}<label className="form-field">Skill ID<input className="resource-code" readOnly={disabled || editor.existing} value={editor.id} aria-invalid={!!errors.id} placeholder="code-review" onChange={e => change({ ...editor, id: e.target.value })} /><small className="muted">A permanent lowercase slug. Cannot be changed after creation.</small>{errors.id && <span role="alert" className="error">{errors.id}</span>}</label>{field("Description", "description", 4)}</section>
        <section className="resource-fields"><h3>✧ Instructions</h3><p className="muted">Tell your agent what to do, and how to do it well.</p>{field("Instructions · Markdown", "content", 14)}</section>
        <section className="resource-fields"><h3>Support files <small>{Object.keys(d.files ?? {}).length} / 32</small></h3><p className="muted">Scripts, references, and templates travel with this skill. Up to 256 KiB per file.</p>{Object.entries(d.files ?? {}).map(([path, content]) => <SupportFile key={path} path={path} content={content} disabled={disabled} rename={to => patch({ files: renameSupportFile(d.files ?? {}, path, to) })} update={value => patch({ files: { ...d.files, [path]: value } })} remove={() => patch({ files: Object.fromEntries(Object.entries(d.files ?? {}).filter(([p]) => p !== path)) })} />)}{!disabled && <div className="resource-inline"><input aria-label="New support file path" placeholder="references/style-guide.md" value={newPath} onChange={e => setNewPath(e.target.value)} /><button type="button" disabled={Object.keys(d.files ?? {}).length >= 32} onClick={() => { if (!validFilePath(newPath) || Object.hasOwn(d.files ?? {}, newPath)) { setFileError("Choose a unique relative file path. SKILL.md is reserved."); return; } patch({ files: { ...d.files, [newPath]: "" } }); setNewPath(""); setFileError(""); }}>+ Add file</button></div>}{(fileError || errors.files) && <p role="alert" className="error">{fileError || errors.files}</p>}</section>
        <section className="resource-fields"><h3>Details & compatibility</h3><div className="resource-columns">{field("Version", "version")}{field("License", "license")}</div>{d.compatibility !== undefined ? <StructuredValueEditor label="Compatibility" value={d.compatibility} disabled={disabled} onChange={compatibility => patch({ compatibility })} onRemove={() => patch({ compatibility: undefined })} /> : !disabled && <button type="button" onClick={() => patch({ compatibility: "" })}>+ Add compatibility</button>}{d.metadata !== undefined ? <StructuredValueEditor label="Metadata" value={d.metadata} objectOnly disabled={disabled} onChange={metadata => { if (metadata && typeof metadata === "object" && !Array.isArray(metadata)) patch({ metadata: Object.fromEntries(Object.entries(metadata)) }); }} onRemove={() => patch({ metadata: undefined })} /> : !disabled && <button type="button" onClick={() => patch({ metadata: {} })}>+ Add metadata</button>}</section>
        <p className="muted">{d.scope === "workspace" ? "Project skill" : "Global skill"} · scope cannot be changed</p>
    </div>;
}
