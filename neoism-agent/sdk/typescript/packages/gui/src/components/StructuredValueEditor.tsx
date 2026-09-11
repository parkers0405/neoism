import { useState } from "react";

export function valueKind(value: unknown): string {
    if (value === null) return "null";
    if (Array.isArray(value)) return "list";
    return typeof value === "object" ? "object" : typeof value;
}
export function initialValue(kind: string): unknown {
    return kind === "object" ? {} : kind === "list" ? [] : kind === "number" ? 0 : kind === "boolean" ? false : kind === "null" ? null : "";
}
function NumberValue({ value, label, onChange }: { value: number; label: string; onChange(value: number): void }) {
    const [draft, setDraft] = useState({ seen: value, text: String(value) });
    if (!Object.is(draft.seen, value)) setDraft({ seen: value, text: String(value) });
    return <input aria-label={label} type="number" step="any" required value={draft.text} onChange={e => {
        const number = e.target.valueAsNumber;
        const valid = Number.isFinite(number);
        setDraft({ seen: valid ? number : value, text: e.target.value });
        e.target.setCustomValidity(valid ? "" : "Enter a finite number.");
        if (valid) onChange(number);
    }} />;
}
export function StructuredValueEditor({ value, onChange, label = "Value", disabled = false, objectOnly = false, onRemove }: {
    value: unknown; onChange(value: unknown): void; label?: string; disabled?: boolean; objectOnly?: boolean; onRemove?(): void;
}) {
    const [key, setKey] = useState("");
    const [error, setError] = useState("");
    const kind = valueKind(value);
    const entries = value && typeof value === "object" && !Array.isArray(value) ? Object.entries(value) : [];
    return <fieldset className="structured-value" disabled={disabled}>
        <legend>{label}</legend>
        <div className="resource-inline">
            {!objectOnly && <label className="form-field">Value type<select aria-label={`${label} type`} value={kind} onChange={e => onChange(initialValue(e.target.value))}>{["string", "number", "boolean", "null", "list", "object"].map(type => <option key={type}>{type}</option>)}</select></label>}
            {onRemove && <button type="button" onClick={onRemove} aria-label={`Remove ${label}`}>Remove</button>}
        </div>
        {kind === "string" && <textarea aria-label={label} rows={2} value={typeof value === "string" ? value : ""} onChange={e => onChange(e.target.value)} />}
        {kind === "number" && <NumberValue label={label} value={typeof value === "number" ? value : 0} onChange={onChange} />}
        {kind === "boolean" && <label><input aria-label={label} type="checkbox" checked={value === true} onChange={e => onChange(e.target.checked)} /> Enabled</label>}
        {kind === "null" && <p className="muted">Null · an explicitly empty value</p>}
        {Array.isArray(value) && <>{value.map((item, index) => <StructuredValueEditor key={index} label={`${label} item ${index + 1}`} value={item} disabled={disabled} onChange={next => onChange(value.map((old, i) => i === index ? next : old))} onRemove={() => onChange(value.filter((_, i) => i !== index))} />)}<button type="button" onClick={() => onChange([...value, ""])}>+ Add item</button></>}
        {kind === "object" && <>{entries.map(([name, item]) => <StructuredValueEditor key={name} label={name} value={item} disabled={disabled} onChange={next => onChange(Object.fromEntries(entries.map(([k, v]) => [k, k === name ? next : v])))} onRemove={() => onChange(Object.fromEntries(entries.filter(([k]) => k !== name)))} />)}
            {!disabled && <div className="resource-inline"><input aria-label={`${label} new property`} placeholder="Property name" value={key} onChange={e => { setKey(e.target.value); setError(""); }} /><button type="button" onClick={() => { if (!key.trim() || entries.some(([k]) => k === key)) { setError("Use a unique, nonempty property name."); return; } onChange(Object.fromEntries([...entries, [key, ""]])); setKey(""); }}>+ Add property</button></div>}
            {error && <p role="alert" className="error">{error}</p>}
        </>}
    </fieldset>;
}
