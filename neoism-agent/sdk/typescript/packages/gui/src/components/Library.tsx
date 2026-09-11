import { useEffect, useState, useRef } from "react";
import {
    workflows,
    NeoismApiError,
    type NeoismClient,
    type WorkflowsClient,
    type OperationResponse,
} from "@neoism/sdk";
import { errorMessage } from "../types";
import { workflowErrors } from "../workflowFormHelpers";
import { useResourceScope } from "../resourceScope";
import { ResourceEditorDialog } from "./ResourceEditorDialog";
import { skillValidation } from "./SkillDefinitionEditor";
import { DefinitionEditor } from "./DefinitionEditor";
import { loadSkillEditor, saveSkillEditor, saveWorkflowEditor, skillDeleteOptions, type Editor } from "../management";
import { SkeletonRows, SkeletonActivity } from "./Skeleton";
import { Modal } from "./Modal";
const installation = { scope: "installation" } as const;
type Skill = OperationResponse<"v2.management.skills.get">;
type Workflow = OperationResponse<"v2.plugins.workflows.get">;
type LibraryProps = {
    kind: "skills" | "workflows";
    client: NeoismClient;
    directory?: string;
    openSession(id: string): void;
};
export function Library(props: LibraryProps) {
    const [identity, setIdentity] = useState({ client: props.client, kind: props.kind, key: 0 });
    if (identity.client !== props.client || identity.kind !== props.kind) {
        setIdentity({ client: props.client, kind: props.kind, key: identity.key + 1 });
    }
    return <ResolvedLibrary key={identity.key} {...props} directory={undefined} />;
}
function ResolvedLibrary(props: LibraryProps) {
    const root = useResourceScope(props.client);
    if (root.error) return <p className="error" role="alert">{root.error} <button onClick={root.retry}>Retry</button></p>;
    return root.loading ? <SkeletonRows kind="setting" count={4} label="Loading global resources…" /> : <ScopedLibrary {...props} canManage={root.canManage} managementReason={root.managementReason} />;
}
function ScopedLibrary({ kind, client, directory, openSession, canManage: capabilityAvailable, managementReason }: LibraryProps & { canManage: boolean; managementReason?: string }) {
    const [authorizationReason, setAuthorizationReason] = useState("");
    const canManage = capabilityAvailable && !authorizationReason;
    const denied = (error: unknown) => { if (error instanceof NeoismApiError && (error.status === 401 || error.status === 403) && alive.current) setAuthorizationReason("The server denied operator authorization. Update your bearer token in Settings, then retry. Your draft has been kept."); };
    const alive = useRef(true);
    useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
    const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
    const [skills, setSkills] = useState<Skill[]>([]);
    const [available, setAvailable] = useState<
        OperationResponse<"v2.skills.list">
    >([]);
    const [flows, setFlows] = useState<Workflow[]>([]);
    const [plugin, setPlugin] = useState<WorkflowsClient>();
    const [error, setError] = useState("");
    const [savedRefreshError, setSavedRefreshError] = useState("");
    const [loading, setLoading] = useState(true);
    const [resolved, setResolved] = useState(false);
    const [editor, setEditor] = useState<Editor>();
    const [history, setHistory] =
        useState<OperationResponse<"v2.plugins.workflows.history">>();
    const run = async (action: () => Promise<void>) => {
        setError("");
        setLoading(true);
        try {
            await action();
            if (!alive.current) return;
        } catch (e) {
            denied(e);
            if (alive.current) setError(errorMessage(e));
        } finally {
            if (alive.current) setLoading(false);
        }
    };
    const refresh = async (strict = false) => {
        if (kind === "skills") {
            try {
                const result = await client.management.skills.list(installation);
                if (!alive.current) return;
                setSkills(result);
                setAvailable([]);
            } catch (error) {
                if (!alive.current) return;
                denied(error);
                if (strict) throw error;
                setSkills([]);
                setError(errorMessage(error));
                const available = await client.operations.request("v2.skills.list", { query: installation });
                if (alive.current) setAvailable(available);
            }
        } else {
            const p = await client.plugins.use(workflows, installation);
            if (!alive.current) return;
            setPlugin(p);
            const result = await p.list(installation);
            if (!alive.current) return;
            setFlows(result.workflows);
            if (result.diagnostics.length)
                setError(result.diagnostics.map((d) => d.message).join("\n"));
        }
    };
    useEffect(() => {
        void run(refresh).finally(() => { if (alive.current) setResolved(true); });
    }, [client, directory, kind]);
    const editSkill = async (s: Skill) => {
        const next = await loadSkillEditor(client, s.id, directory);
        if (alive.current) { setFieldErrors({}); setEditor(canManage ? next : { ...next, readOnly: true, reason: managementReason }); }
    };
    // A refresh failure cannot undo a committed write or reopen a stale draft.
    const refreshAfterSave = async () => {
        try {
            await refresh(true);
            if (alive.current) setSavedRefreshError("");
        } catch (error) {
            denied(error);
            if (alive.current) setSavedRefreshError(errorMessage(error));
        }
    };
    const save = async () => {
        if (!editor || editor.readOnly || !canManage) return;
        if (editor.kind === "skills") {
            const errors = skillValidation(editor); setFieldErrors(errors);
            if (Object.keys(errors).length) return;
        }
        if (editor.kind === "skills") {
            const saved = await saveSkillEditor(client, editor, directory);
            if (!alive.current) return;
            // Adopt the server's committed projection (including revision), never the draft.
            setSkills(current => [...current.filter(skill => skill.id !== saved.id || skill.scope !== saved.scope), saved]);
            setAvailable([]);
        } else {
            const errors = workflowErrors(editor.definition);
            if (errors.length) throw new Error(errors.join("\n"));
            if (!plugin) throw new Error("Workflows capability unavailable");
            const saved = await saveWorkflowEditor(plugin, editor, directory);
            if (!alive.current) return;
            setFlows(current => [...current.filter(flow => flow.definition.id !== saved.definition.id), saved]);
        }
        setEditor(undefined);
        await refreshAfterSave();
    };
    return (
        <section className="library">
            <div className="page-heading">
                <div>
                    <h1>{kind === "skills" ? "Global skills" : "Global workflows"}</h1>
                    <p>
                        {kind === "skills"
                            ? "Reusable knowledge for your agent."
                            : "Turn repeatable work into a rhythm."}
                    </p>
                </div>
                <button
                    className="primary"
                    disabled={loading || !canManage || (kind === "workflows" && !plugin)}
                    title={!canManage ? authorizationReason || managementReason : undefined}
                    onClick={() => {
                        setFieldErrors({}); setError("");
                        setEditor(kind === "skills" ? {
                            kind, id: "", existing: false,
                            definition: { name: "", description: "", content: "# My skill\n\nInstructions for the agent.", scope: "installation", files: {} },
                        } : {
                            kind, id: "", existing: false,
                            definition: { id: "my-workflow", name: "My workflow", active: false, prompt: "", directory,
                                schedule: { frequency: "daily", interval: 1, time: "09:00", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone } },
                        });
                    }}
                >
                    + New {kind === "skills" ? "skill" : "workflow"}
                </button>
            </div>
            <p className="notice">{authorizationReason || managementReason}</p>
            {kind === "workflows" && <p className="notice">New workflows start paused.</p>}
            {error && (
                <div role="alert" className="error">
                    {error}
                    <button onClick={() => { setAuthorizationReason(""); void run(refresh); }}>Retry</button>
                </div>
            )}
            {savedRefreshError && <div role="alert" className="notice">
                Saved, but the list could not refresh. {savedRefreshError}
                <button disabled={loading} onClick={() => { setAuthorizationReason(""); void run(refreshAfterSave); }}>Retry refresh</button>
            </div>}
            {!resolved && <SkeletonRows kind="setting" count={4} label={`Loading ${kind}…`} />}
            {resolved && loading && <SkeletonActivity label={`Updating ${kind}…`} />}
            {available.length > 0 && kind === "skills" && (
                <>
                    <h3>AVAILABLE SKILLS · READ ONLY</h3>
                    <div className="resource-grid">
                        {available.map((skill) => (
                            <article className="resource" key={skill.name}>
                                <h2>{skill.name}</h2>
                                <p>{skill.description}</p>
                                <small>{skill.path}</small>
                                <button onClick={() => setEditor({ kind: "skills", id: skill.name, existing: true, readOnly: true, reason: "Catalog details only. The server has not exposed the complete instructions or support files for this read-only resource.", definition: { name: skill.name, description: skill.description ?? "", content: "" } })}>View details</button>
                            </article>
                        ))}
                    </div>
                </>
            )}
            <div className="resource-grid">
                {kind === "skills"
                    ? skills.map((s) => (
                          <article className="resource" key={s.id}>
                              <h2>{s.id}</h2>
                              <p>
                                  {s.origin} · {s.scope || "read-only"}
                              </p>
                              <div className="actions">
                                  <button
                                      onClick={() =>
                                          void run(() => editSkill(s))
                                      }
                                  >
                                      {s.writable && canManage ? "Edit" : "View details"}
                                  </button>
                                  <button
                                      disabled={!canManage || !s.writable || loading}
                                      onClick={() => {
                                          if (confirm(`Delete skill ${s.id}?`))
                                              void run(async () => {
                                                  await client.management.skills.delete(
                                                      s.id, skillDeleteOptions(s, directory),
                                                  );
                                                  await refresh(true);
                                              });
                                      }}
                                  >
                                      Delete
                                  </button>
                              </div>
                          </article>
                      ))
                    : flows.map((f) => (
                          <article className="resource" key={f.definition.id}>
                              <h2>{f.definition.name}</h2>
                              <p>
                                  {f.active ? "Active" : "Paused"} ·{" "}
                                  {f.definition.schedule.frequency}
                              </p>
                              <p>{f.definition.prompt}</p>
                              <div className="actions">
                                  <button
                                      onClick={() =>
                                          setEditor({
                                              id: f.definition.id,
                                              kind: "workflows",
                                              scope: "installation",
                                              definition: f.definition,
                                              existing: true,
                                              revision: f.revision,
                                              readOnly: !f.writable || !canManage,
                                              reason: !f.writable ? "This workflow is read-only." : !canManage ? managementReason : undefined,
                                          })
                                      }
                                  >
                                      {f.writable && canManage ? "Edit" : "View details"}
                                  </button>
                                  <button
                                      disabled={loading || !canManage || !f.writable}
                                      onClick={() =>
                                          void run(async () => {
                                              await plugin!.run(
                                                  f.definition.id,
                                                  installation,
                                              );
                                              setHistory(
                                                  await plugin!.history(
                                                      f.definition.id,
                                                      installation,
                                                  ),
                                              );
                                          })
                                      }
                                  >
                                      Run now
                                  </button>
                                  <button
                                      disabled={loading || !canManage || !f.writable}
                                      onClick={() =>
                                          void run(async () => {
                                              await (f.active
                                                  ? plugin!.pause(
                                                        f.definition.id,
                                                        installation,
                                                    )
                                                  : plugin!.activate(
                                                        f.definition.id,
                                                        installation,
                                                    ));
                                              await refresh(true);
                                          })
                                      }
                                  >
                                      {f.active ? "Pause" : "Activate"}
                                  </button>
                                  <button
                                      onClick={() =>
                                          void run(async () =>
                                              setHistory(
                                                  await plugin!.history(
                                                      f.definition.id,
                                                      { ...installation, limit: 50 },
                                                  ),
                                              ),
                                          )
                                      }
                                  >
                                      History
                                  </button>
                                  <button
                                      disabled={!canManage || !f.writable || loading}
                                      onClick={() => {
                                          if (
                                              confirm(
                                                  `Delete workflow ${f.definition.name}?`,
                                              )
                                          )
                                              void run(async () => {
                                                  await plugin!.remove(
                                                      f.definition.id,
                                                      {
                                                          ...installation,
                                                          revision: f.revision,
                                                      },
                                                  );
                                                  await refresh(true);
                                              });
                                      }}
                                  >
                                      Delete
                                  </button>
                              </div>
                          </article>
                      ))}
            </div>
            {resolved && !loading &&
                !error &&
                (kind === "skills" ? skills.length + available.length : flows.length) === 0 && (
                    <div className="empty">
                        Nothing here yet. Create your first{" "}
                        {kind === "skills" ? "skill" : "workflow"}.
                    </div>
                )}
            {editor && (
                <ResourceEditorDialog kind={kind} existing={editor.existing} readOnly={editor.readOnly || !canManage} directory={directory} busy={loading} close={() => setEditor(undefined)} submit={() => void run(save)}>
                    {editor.reason && <p role="status" className="notice">{editor.reason}</p>}
                    <DefinitionEditor editor={canManage ? editor : { ...editor, readOnly: true }} change={setEditor} client={client} directory={directory} errors={fieldErrors} />
                    {editor.kind === "skills" && editor.existing && <SkillVersions client={client} editor={editor} directory={directory} canManage={canManage} onBusy={setLoading} onDenied={denied} restored={async () => { await refresh(true); const next = await loadSkillEditor(client, editor.id, directory); if (alive.current) setEditor(next); }} />}
                    {error && <p role="alert" className="error">{error}</p>}
                    {authorizationReason && <p className="notice" role="alert">{authorizationReason} <button type="button" onClick={() => { setAuthorizationReason(""); void run(refresh); }}>Retry authorization</button></p>}
                </ResourceEditorDialog>
            )}
            {history && (
                <Modal title="Run history" close={() => setHistory(undefined)}>
                    {history.runs.length ? (
                        history.runs.map((r) => (
                            <article className="history-row" key={r.id}>
                                <strong>{r.status}</strong>
                                <span>
                                    {new Date(r.created).toLocaleString()}
                                </span>
                                {r.error && <p className="error">{r.error}</p>}
                                {r.sessionId && (
                                    <button
                                        onClick={() =>
                                            openSession(r.sessionId!)
                                        }
                                    >
                                        Open chat →
                                    </button>
                                )}
                            </article>
                        ))
                    ) : (
                        <p className="empty">No runs yet.</p>
                    )}
                </Modal>
            )}
        </section>
    );
}

function SkillVersions({ client, editor, directory, canManage, restored, onBusy, onDenied }: {
    client: NeoismClient; editor: Extract<Editor, { kind: "skills" }>; directory?: string; canManage: boolean; restored(): Promise<void>; onBusy(busy: boolean): void; onDenied(error: unknown): void;
}) {
    const [versions, setVersions] = useState<OperationResponse<"v2.management.skills.versions.list">>();
    const [view, setView] = useState<OperationResponse<"v2.management.skills.versions.get">>();
    const [error, setError] = useState("");
    const [busy, setBusy] = useState(false);
    const active = useRef(true);
    useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
    const action = async (work: () => Promise<void>) => {
        setBusy(true); onBusy(true); setError("");
        try { await work(); } catch (e) { onDenied(e); if (active.current) setError(errorMessage(e)); }
        finally { if (active.current) { setBusy(false); onBusy(false); } }
    };
    return <details className="resource-fields"><summary>Version history · view & restore</summary><p className="muted">Restoring replaces the entire bundle, including support files. Your current revision is checked before writing.</p><button type="button" disabled={busy} onClick={() => void action(async () => { const list = await client.management.skills.versions(editor.id, installation); if (active.current) setVersions(list.filter(v => v.skillId === editor.id && v.scope === editor.scope)); })}>{versions ? "Refresh versions" : "Load versions"}</button>
        {versions && <div className="resource-version-list">{versions.length ? versions.map(v => <div className="resource-inline" key={v.id}><code>{v.id}</code><span>{new Date(v.createdAt).toLocaleString()}</span>{v.revision === editor.revision && <small>Current</small>}<button type="button" disabled={busy} onClick={() => void action(async () => { const full = await client.management.skills.version(editor.id, v.id, installation); if (full.skillId !== editor.id || full.scope !== editor.scope || full.revision !== v.revision) throw Error("Version identity mismatch; reload history."); if (active.current) setView(full); })}>View version</button><button type="button" disabled={busy || editor.readOnly || !canManage || !editor.revision || v.revision === editor.revision} onClick={() => { if (confirm(`Restore version ${v.id}? This replaces all instructions, metadata and files, and discards unsaved changes in this editor.`)) void action(async () => { const current = await client.management.skills.get(editor.id, installation); if (current.scope !== editor.scope || current.revision !== editor.revision || !current.writable) throw Error("This skill changed or is no longer writable. Close and reopen it before restoring."); await client.management.skills.restore(editor.id, v.id, { ...installation, expectedRevision: editor.revision }); if (active.current) { setView(undefined); await restored(); } }); }}>Restore</button></div>) : <p>No saved versions.</p>}</div>}
        {view && <section><h3>Version {view.id}</h3><DefinitionEditor key={view.id} editor={{ kind: "skills", id: editor.id, existing: true, readOnly: true, scope: view.scope, revision: view.revision, definition: view.bundle }} change={() => {}} /><button type="button" onClick={() => setView(undefined)}>Close version preview</button></section>}
        {error && <p role="alert" className="error">{error}</p>}
    </details>;
}
