import { useEffect, useMemo, useRef, useState } from "react";
import { workflows, type NeoismClient, type WorkflowDefinition, type WorkflowPreview, type ProviderConnectionSummary } from "@neoism/sdk";
import { WorkflowChoice } from "./WorkflowChoice";
import { frequencies, isRules, loadWorkflowCatalogs, onceMode, patchWorkflow, retryDefaults, scopeGuard, timezones, weekdays, workflowErrors, workflowFrequency } from "../workflowFormHelpers";
import "./workflow-editor.css";

export interface WorkflowDefinitionEditorProps {
    value: WorkflowDefinition;
    onChange(value: WorkflowDefinition): void;
    /** Current validation message, or an empty string when valid. */
    onError?(message: string): void;
    editing?: boolean;
    client?: NeoismClient;
    /** Definition storage/project root; used for catalogs and saved preview only. */
    directory?: string;
    disabled?: boolean;
}

function Chips({ label, values, onChange, disabled }: { label: string; values: string[]; onChange(values: string[]): void; disabled?: boolean }) {
    const [draft, setDraft] = useState("");
    const add = () => { if (draft.trim()) { onChange([...values, draft.trim()]); setDraft(""); } };
    return <fieldset className="workflow-chips" disabled={disabled}><legend>{label}</legend>
        <div className="workflow-chip-list">{values.map((v, i) => <span className="workflow-chip" key={i}>
            <input aria-label={`${label} ${i + 1}`} value={v} onChange={e => onChange(values.map((s, n) => n === i ? e.target.value : s))} />
            <button type="button" aria-label={`Remove ${label} ${i + 1}`} onClick={() => onChange(values.filter((_, n) => n !== i))}>×</button>
        </span>)}</div>
        <div className="workflow-inline"><input aria-label={`New ${label}`} value={draft} placeholder="Add a value…" onChange={e => setDraft(e.target.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); add(); } }} /><button type="button" disabled={!draft.trim()} onClick={add}>Add</button></div>
    </fieldset>;
}

function Permissions({ value, onChange, disabled }: { value: WorkflowDefinition["permissions"]; onChange(value: NonNullable<WorkflowDefinition["permissions"]>): void; disabled: boolean }) {
    const [name, setName] = useState("");
    const rules = value ?? {};
    const update = (key: string, rule: unknown) => onChange({ ...rules, [key]: rule });
    const remove = (key: string) => { const next = { ...rules }; delete next[key]; onChange(next); };
    return <section className="resource-section"><h3>Permissions</h3><p className="muted">Scheduled runs cannot answer permission questions. Use allow or deny, optionally restricted by patterns. Unspecified permissions follow server policy.</p>
        {Object.entries(rules).map(([key, rule]) => {
            const structured = isRules(rule);
            const mode = structured ? "patterns" : typeof rule === "string" ? rule : "invalid";
            const field = (fieldName: string, v: unknown) => update(key, { ...(structured ? rule : {}), [fieldName]: v });
            return <fieldset className="workflow-permission" disabled={disabled} key={key}><legend>{key || "Unnamed permission"}</legend>
                <div className="workflow-inline"><label className="form-field">Permission name<input defaultValue={key} onChange={e => e.target.setCustomValidity(e.target.value !== key && Object.hasOwn(rules, e.target.value) ? "This permission name already exists." : "")} onBlur={e => { const nextName = e.target.value; if (nextName !== key && !Object.hasOwn(rules, nextName)) onChange(Object.fromEntries(Object.entries(rules).map(([name, setting]) => [name === key ? nextName : name, setting]))); }} /></label>
                <label className="form-field">Rule<select value={mode} onChange={e => update(key, e.target.value === "patterns" ? { default: rule === "allow" || rule === "deny" || rule === "ask" ? rule : "deny", allow: [], deny: [] } : e.target.value)}>
                    <option value="allow">Allow</option><option value="deny">Deny</option><option value="patterns">Pattern rules</option>{!["allow", "deny", "patterns"].includes(mode) && <option value={mode}>{mode} (invalid for schedules)</option>}
                </select></label><button type="button" onClick={() => remove(key)}>Remove permission</button></div>
                {structured && <>
                    <label className="form-field">Default action<select value={typeof rule.default === "string" ? rule.default : ""} onChange={e => { const next = { ...rule }; if (e.target.value) next.default = e.target.value; else delete next.default; update(key, next); }}><option value="">No default</option><option value="allow">Allow</option><option value="deny">Deny</option>{rule.default !== undefined && rule.default !== "allow" && rule.default !== "deny" && <option value={String(rule.default)}>{String(rule.default)} (invalid)</option>}</select></label>
                    {(["allow", "deny"] as const).map(action => <Chips key={action} label={`${action === "allow" ? "Allowed" : "Denied"} patterns`} values={Array.isArray(rule[action]) ? rule[action].filter((v): v is string => typeof v === "string") : []} disabled={disabled} onChange={v => field(action, v)} />)}
                    {Object.hasOwn(rule, "ask") && <div className="workflow-warning"><p>Existing ask patterns: {Array.isArray(rule.ask) ? rule.ask.map(String).join(" · ") || "(empty)" : String(rule.ask)}. Non-empty ask rules are not valid for schedules.</p><button type="button" onClick={() => { const next = { ...rule }; delete next.ask; update(key, next); }}>Remove ask rules</button></div>}
                    {Object.keys(rule).filter(k => !["default", "allow", "deny", "ask"].includes(k)).map(k => <p key={k}>Unsupported rule key: {k} <button type="button" onClick={() => { const next = { ...rule }; delete next[k]; update(key, next); }}>Remove {k}</button></p>)}
                    {["allow", "deny"].filter(k => rule[k] !== undefined && (!Array.isArray(rule[k]) || rule[k].some(v => typeof v !== "string"))).map(k => <p role="alert" key={k}>Invalid {k} pattern data. <button type="button" onClick={() => field(k, [])}>Clear invalid {k} patterns</button></p>)}
                </>}
                {rule === "ask" && <p role="alert">Ask is not valid for schedules. Choose allow/deny above or remove this permission.</p>}
            </fieldset>;
        })}
        <div className="workflow-inline"><label className="form-field">New permission name<input disabled={disabled} value={name} placeholder="e.g. bash, edit, webfetch, *" onChange={e => setName(e.target.value)} /></label><button type="button" disabled={disabled || !name.trim() || Object.hasOwn(rules, name.trim())} onClick={() => { update(name.trim(), "deny"); setName(""); }}>Add permission</button></div>
    </section>;
}

export function WorkflowDefinitionEditor({ value: d, onChange, onError, editing = false, client, directory, disabled = false }: WorkflowDefinitionEditorProps) {
    const form = useRef<HTMLFieldSetElement>(null);
    const patch = (changes: Partial<WorkflowDefinition>) => { if (!disabled && !form.current?.closest("fieldset[disabled]")) onChange(patchWorkflow(d, changes)); };
    const s = d.schedule;
    const schedule = (changes: Partial<typeof s>) => patch({ schedule: { ...s, ...changes } });
    const retry = { ...retryDefaults, ...d.retry };
    const zones = useMemo(timezones, []);
    const errors = workflowErrors(d);
    const errorMessage = errors.join("\n");
    const validity = useRef<HTMLInputElement>(null);
    useEffect(() => { validity.current?.setCustomValidity(errorMessage); onError?.(errorMessage); }, [errorMessage, onError]);
    const [catalog, setCatalog] = useState<Awaited<ReturnType<typeof loadWorkflowCatalogs>>>();
    const [catalogError, setCatalogError] = useState("");
    const [accounts, setAccounts] = useState<ProviderConnectionSummary[]>([]);
    const [accountError, setAccountError] = useState("");
    const [reload, setReload] = useState(0);
    useEffect(() => {
        const guard = scopeGuard(); setCatalog(undefined); setCatalogError("");
        if (client) void guard.run(loadWorkflowCatalogs(client, directory), setCatalog, e => setCatalogError(String(e)));
        return guard.cancel;
    }, [client, directory, reload]);
    const providerId = d.model?.providerId;
    useEffect(() => {
        const guard = scopeGuard(); setAccounts([]); setAccountError("");
        if (client && providerId) void guard.run((async () => {
            // The account API expects workspace ID, not a filesystem directory.
            const workspaces = directory ? await client.management.workspaces.list() : [];
            const workspace = workspaces.find(w => w.root === directory);
            if (directory && !workspace) throw Error("Workspace account scope could not be resolved. Enter an account ID if needed.");
            return client.catalog.providers.connections(providerId, workspace?.id);
        })(), setAccounts, e => setAccountError(String(e)));
        return guard.cancel;
    }, [client, directory, providerId, reload]);
    const [preview, setPreview] = useState<WorkflowPreview>();
    const [previewStatus, setPreviewStatus] = useState("");
    const [previewRequest, setPreviewRequest] = useState(0);
    useEffect(() => {
        const guard = scopeGuard(); setPreview(undefined); setPreviewStatus("");
        if (client && editing && previewRequest) {
            setPreviewStatus("Loading saved schedule…");
            void guard.run((async () => { const context = directory ? { directory, scope: "workspace" as const } : { scope: "installation" as const }; const plugin = await client.plugins.use(workflows, context); if (!plugin || typeof plugin.preview !== "function") throw Error("Schedule preview is not supported by this server."); return plugin.preview(d.id, context); })(), result => { setPreview(result); setPreviewStatus(""); }, e => setPreviewStatus(String(e)));
        }
        return guard.cancel;
    }, [client, directory, editing, d.id, previewRequest]);
    const provider = catalog?.providers.find(p => p.id === providerId);
    const model = Object.values(provider?.models ?? {}).find(m => m.id === d.model?.id);
    const variants = isRules(model?.variants) ? Object.keys(model.variants) : [];
    const text = (label: string, value: string | undefined, change: (v: string) => void, type = "text", locked = false) => <label className="form-field">{label}<input type={type} disabled={disabled || locked} value={value ?? ""} onChange={e => change(e.target.value)} /></label>;
    const number = (label: string, value: number, change: (v: number) => void, min = 1, max?: number, locked = false) => <label className="form-field">{label}<input type="number" step={1} min={min} max={max} required disabled={disabled || locked} value={Number.isFinite(value) ? value : ""} onChange={e => change(e.target.valueAsNumber)} /></label>;
    const select = (label: string, value: string, options: readonly string[], change: (v: string) => void) => <label className="form-field">{label}<select disabled={disabled} value={value} onChange={e => change(e.target.value)}>{!options.includes(value) && <option value={value}>{value} (current)</option>}{options.map(o => <option key={o}>{o}</option>)}</select></label>;
    return <fieldset ref={form} disabled={disabled} className="definition-form workflow-editor">
        <section className="resource-section">
            <div className="form-columns">{text("Workflow ID", d.id, id => patch({ id }), "text", editing)}{text("Name", d.name, name => patch({ name }))}</div>
            <label className="form-field">Prompt<textarea rows={10} disabled={disabled} value={d.prompt} placeholder="What should the agent do each time this workflow runs?" onChange={e => patch({ prompt: e.target.value })} /></label>
            <label className="workflow-checkbox"><input type="checkbox" disabled={disabled} checked={d.active ?? false} onChange={e => patch({ active: e.target.checked })} />Enable schedule</label>
            <p className="muted">Leave disabled to save without scheduling automatic runs.</p>
        </section>
        <section className="resource-section"><h3>Schedule</h3><div className="form-columns">
            {select("Frequency", s.frequency, frequencies, frequency => patch({ schedule: workflowFrequency(s, frequency) }))}
            {s.frequency !== "once" && number("Every (interval)", s.interval, interval => schedule({ interval }), 1, 4294967295)}
            {s.frequency === "hourly" && number("Minute of the hour", s.minute ?? 0, minute => schedule({ minute }), 0, 59)}
            {s.frequency === "once" && select("One-time date mode", s.at !== undefined ? "ISO timestamp" : "Date and time", ["Date and time", "ISO timestamp"], mode => patch({ schedule: onceMode(s, mode === "ISO timestamp") }))}
            {s.frequency === "once" && (s.at !== undefined ? text("ISO timestamp (with seconds and offset)", s.at, at => schedule({ at })) : text("Date", s.date, date => schedule({ date }), "date"))}
            {s.frequency !== "hourly" && s.at === undefined && text("Time (in selected timezone)", s.time ?? "00:00", time => schedule({ time }))}
            {s.frequency === "monthly" && number("Day of month", s.monthDay ?? 1, monthDay => schedule({ monthDay }), 1, 31)}
            <WorkflowChoice label="Timezone" value={s.timezone} options={zones} optional={false} allowCustom disabled={disabled} onChange={timezone => schedule({ timezone })} />
        </div>
        {s.frequency === "weekly" && <fieldset className="workflow-weekdays" disabled={disabled}><legend>Run on</legend>{weekdays.map(day => { const selected = s.weekdays?.some(w => w.toLowerCase().slice(0, 3) === day.slice(0, 3)) ?? false; return <button type="button" key={day} aria-pressed={selected} onClick={() => schedule({ weekdays: selected ? (s.weekdays ?? []).filter(w => w.toLowerCase().slice(0, 3) !== day.slice(0, 3)) : [...(s.weekdays ?? []), day] })}>{day.slice(0, 3)}</button>; })}</fieldset>}
        {s.frequency === "monthly" && <p className="muted">Days 29–31 do not occur in every month. Check the server preview for actual runs.</p>}
        <div className="workflow-preview"><button type="button" disabled={disabled || !client || !editing} onClick={() => setPreviewRequest(n => n + 1)}>Preview saved schedule</button><p className="muted">Preview uses the saved definition, not unsaved form changes. Save first to preview a new schedule.</p>
        {previewStatus && <p role="status">{previewStatus}</p>}{preview && <><p>Next runs · {preview.definition.schedule.timezone} (saved)</p>{preview.upcoming.length ? <ol>{preview.upcoming.map((run, i) => <li key={`${run.scheduledAt}-${i}`}>{run.local}</li>)}</ol> : <p>No upcoming runs returned by the server.</p>}</>}
        </div></section>
        <section className="resource-section"><h3>Options</h3>
            {text("Execution directory (optional)", d.directory, directory => patch({ directory: directory.trim() ? directory : undefined }))}
            <p className="muted">Blank uses the default execution location. {directory ? "The definition stays in the selected project." : "The definition stays global."}</p>
            {!client && <p className="muted">Connect a client to browse agents, skills and configured models. Existing selections are retained.</p>}
            {client && !catalog && !catalogError && <p role="status">Loading catalogs…</p>}
            {catalogError && <p role="status">Catalogs unavailable: {catalogError} <button type="button" disabled={disabled} onClick={() => setReload(n => n + 1)}>Retry catalogs</button></p>}
            <div className="form-columns"><WorkflowChoice label="Agent" value={d.agent ?? ""} options={catalog?.agents.map(a => ({ value: a.name, label: a.name })) ?? []} disabled={disabled} onChange={agent => patch({ agent: agent || undefined })} />
            <WorkflowChoice label="Skill" value={d.skill ?? ""} options={catalog?.skills.map(s => ({ value: s.name, label: s.name })) ?? []} disabled={disabled} onChange={skill => patch({ skill: skill || undefined })} />
            <WorkflowChoice label="Model provider" value={providerId ?? ""} options={catalog?.providers.map(p => ({ value: p.id, label: p.name })) ?? []} disabled={disabled} onChange={providerId => patch({ model: providerId ? { ...d.model, id: d.model?.id ?? "", providerId } : undefined })} />
            <WorkflowChoice label="Model ID" value={d.model?.id ?? ""} options={Object.values(provider?.models ?? {}).map(m => ({ value: m.id, label: m.name }))} disabled={disabled || !d.model} onChange={id => { if (d.model) patch({ model: { ...d.model, id } }); }} />
            <WorkflowChoice label="Account / connection ID" value={d.model?.connectionId ?? ""} options={accounts.map(a => ({ value: a.connectionId, label: `${a.label}${a.isDefault ? " (default)" : ""}` }))} allowCustom disabled={disabled || !d.model} onChange={connectionId => { if (d.model) patch({ model: { ...d.model, connectionId: connectionId || undefined } }); }} />
            <WorkflowChoice label="Model variant" value={d.model?.variant ?? ""} options={variants} allowCustom disabled={disabled || !d.model} onChange={variant => { if (d.model) patch({ model: { ...d.model, variant: variant || undefined } }); }} /></div>
            {accountError && <p className="muted" role="status">Accounts unavailable: {accountError}</p>}
            <p className="muted">Model IDs, accounts and variants already selected are kept even when absent from the catalog. Clear the provider to use the default model.</p>
        </section>
        <section className="resource-section"><h3>Concurrency</h3><div className="form-columns">
            {select("When a run is still active", d.concurrency?.mode ?? "forbid", ["forbid", "replace", "allow"], mode => patch({ concurrency: { mode: mode as "forbid" | "replace" | "allow", maxRunning: mode === "allow" ? d.concurrency?.maxRunning ?? 1 : 1 } }))}
            {number("Maximum running", d.concurrency?.maxRunning ?? 1, maxRunning => patch({ concurrency: { ...d.concurrency, maxRunning } }), 1, 4294967295, (d.concurrency?.mode ?? "forbid") !== "allow")}
        </div><p className="muted">Forbid skips overlapping runs. Replace supersedes the current run. Allow permits concurrent runs up to your limit.</p></section>
        <section className="resource-section"><h3>Retry policy</h3><div className="form-columns">
            {number("Maximum attempts (including first run)", retry.maxAttempts, maxAttempts => patch({ retry: { ...d.retry, maxAttempts } }), 1, 4294967295)}
            {select("Backoff", retry.backoff, ["fixed", "exponential"], backoff => patch({ retry: { ...d.retry, backoff: backoff as "fixed" | "exponential" } }))}
            {number("Initial retry delay (ms)", retry.initialDelayMs, initialDelayMs => patch({ retry: { ...d.retry, initialDelayMs } }), 0)}
            {number("Maximum retry delay (ms; 0 = unbounded)", retry.maxDelayMs, maxDelayMs => patch({ retry: { ...d.retry, maxDelayMs } }), 0)}
        </div><Chips label="Retryable error codes" values={retry.retryableErrors} disabled={disabled} onChange={retryableErrors => patch({ retry: { ...d.retry, retryableErrors } })} /><p className="muted">One attempt means no retries. Native defaults are zero delay and no delay cap.</p></section>
        <Permissions value={d.permissions} disabled={disabled} onChange={permissions => patch({ permissions })} />
        {errors.length > 0 && <div className="workflow-validation" role="alert"><strong>Before saving</strong><ul>{errors.map((error, i) => <li key={i}>{error}</li>)}</ul></div>}
        <input ref={validity} className="workflow-validity" aria-label="Workflow validation" value="" onChange={() => {}} tabIndex={-1} />
    </fieldset>;
}

export default WorkflowDefinitionEditor;
