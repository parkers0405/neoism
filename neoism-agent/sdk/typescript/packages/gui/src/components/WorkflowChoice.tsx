import { useId, useState } from "react";

export interface WorkflowChoiceOption { value: string; label?: string }
export interface WorkflowChoiceProps {
    label: string;
    value: string;
    options: readonly (string | WorkflowChoiceOption)[];
    onChange(value: string): void;
    disabled?: boolean;
    optional?: boolean;
    /** Account/variant IDs may not be discoverable on older servers. */
    allowCustom?: boolean;
}
/** Native keyboard/screen-reader selection with an explicit searchable catalog. */
export function WorkflowChoice({ label, value, options, onChange, disabled, optional = true, allowCustom = false }: WorkflowChoiceProps) {
    const id = useId();
    const [search, setSearch] = useState("");
    const choices = options.map(o => typeof o === "string" ? { value: o, label: o } : o);
    const known = choices.some(o => o.value === value);
    const filtered = choices.filter(o => o.value === value || `${o.label ?? ""} ${o.value}`.toLowerCase().includes(search.toLowerCase()));
    return <div className="form-field workflow-choice">
        <label htmlFor={id}>{label}</label>
        {allowCustom ? <><input id={id} disabled={disabled} list={`${id}-list`} value={value} placeholder="Default / search or enter ID" onChange={e => onChange(e.target.value)} /><datalist id={`${id}-list`}>{choices.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}</datalist></> : <>
            <input type="search" aria-label={`Search ${label}`} disabled={disabled} value={search} placeholder={`Search ${label.toLowerCase()}…`} onChange={e => setSearch(e.target.value)} />
            <select id={id} disabled={disabled} value={value} onChange={e => onChange(e.target.value)}>
                {optional && <option value="">Default / none</option>}
                {value && !known && <option value={value}>{value} (current · not in catalog)</option>}
                {filtered.map(o => <option key={o.value} value={o.value}>{o.label ?? o.value}</option>)}
            </select>
            {!filtered.length && <small className="muted">No matching catalog options. Existing values are retained.</small>}
        </>}
    </div>;
}
