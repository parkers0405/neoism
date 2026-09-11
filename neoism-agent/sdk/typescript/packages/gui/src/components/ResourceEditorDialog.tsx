import { useEffect, useId, useRef, type ReactNode } from "react";
import { Sparkles, Workflow, X } from "lucide-react";
import "../resource-editor.css";
export function ResourceEditorDialog({ kind, existing, readOnly, directory, busy, children, close, submit }: {
    kind: "skills" | "workflows"; existing: boolean; readOnly?: boolean; directory?: string; busy: boolean; children: ReactNode; close(): void; submit(): void;
}) {
    const ref = useRef<HTMLDialogElement>(null);
    const titleId = useId();
    const noun = kind === "skills" ? "skill" : "workflow";
    useEffect(() => {
        const previous = document.activeElement;
        const dialog = ref.current!;
        dialog.showModal();
        return () => { dialog.close(); if (previous instanceof HTMLElement) previous.focus(); };
    }, []);
    return <dialog ref={ref} className="resource-editor-dialog" aria-labelledby={titleId} onCancel={e => { if (busy) e.preventDefault(); else close(); }}>
        <form className="resource-editor-shell" onSubmit={e => { e.preventDefault(); submit(); }}>
            <header className="resource-editor-heading"><div className="resource-heading-title"><span className="resource-glyph" aria-hidden="true">{kind === "skills" ? <Sparkles size={24} /> : <Workflow size={24} />}</span><div><h2 id={titleId}>{readOnly ? "View" : existing ? "Edit" : "New"} {noun}</h2><p>{kind === "skills" ? "A little expertise, ready whenever you need it." : "Give repeatable work a rhythm of its own."}</p></div></div><button type="button" aria-label="Close resource editor" disabled={busy} onClick={close}><X size={20} /></button></header>
            <div className="resource-editor-body"><div className="resource-destination"><span>{directory ? "Selected project" : kind === "skills" ? "Global skills" : "Global workflows"}</span>{directory && <code>{directory}</code>}<small>{existing ? "Scope cannot be changed" : "Available across projects"}</small></div><fieldset className="resource-editable" disabled={busy}>{children}</fieldset></div>
            <footer className="resource-editor-actions"><span className="muted">{readOnly ? "Read-only resource" : busy ? "Saving changes…" : "Changes are saved only when you confirm."}</span><button type="button" disabled={busy} onClick={close}>{readOnly ? "Close" : "Cancel"}</button>{!readOnly && <button type="submit" className="primary" disabled={busy}>{existing ? "Save changes" : kind === "skills" ? "Create skill" : "Create workflow"}</button>}</footer>
        </form>
    </dialog>;
}
