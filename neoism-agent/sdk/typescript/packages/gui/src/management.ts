import type { WorkflowsClient, NeoismClient, OperationInput, OperationResponse } from "@neoism/sdk";
export type SkillWrite = OperationInput<"v2.management.skills.update">["body"];
export type FlowWrite = OperationInput<"v2.plugins.workflows.create">["body"];
export type Skill = OperationResponse<"v2.management.skills.get">;
export type Editor = { id: string; existing: boolean; revision?: string; reason?: string } & (
    { kind: "skills"; definition: SkillWrite; scope?: Skill["scope"]; readOnly?: boolean } |
    { kind: "workflows"; definition: FlowWrite; scope?: "installation" | "workspace"; readOnly?: boolean }
);
function record(value: unknown): Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : {};
}
function object(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}
// ManagedResource.definition is intentionally generic. Validate the additive
// projection at the boundary instead of casting catalog data to a write body.
export function parseSkillBundle(value: unknown): SkillWrite | undefined {
    if (!object(value) || typeof value.content !== "string" ||
        (value.scope !== "workspace" && value.scope !== "installation") ||
        typeof value.expectedRevision !== "string" || !object(value.files)) return;
    const allowed = new Set(["content", "scope", "expectedRevision", "files", "name", "description", "version", "license", "metadata", "compatibility"]);
    if (Object.keys(value).some(key => !allowed.has(key))) return;
    for (const key of ["name", "description", "version", "license"]) {
        if (value[key] != null && typeof value[key] !== "string") return;
    }
    if (value.metadata !== undefined && !object(value.metadata)) return;
    const files: Record<string, string> = {};
    for (const [path, content] of Object.entries(value.files)) {
        if (typeof content !== "string") return;
        Object.defineProperty(files, path, { value: content, enumerable: true, writable: true, configurable: true });
    }
    return {
        content: value.content, scope: value.scope, expectedRevision: value.expectedRevision, files,
        name: typeof value.name === "string" ? value.name : undefined,
        description: typeof value.description === "string" ? value.description : undefined,
        version: typeof value.version === "string" ? value.version : undefined,
        license: typeof value.license === "string" ? value.license : undefined,
        metadata: object(value.metadata) ? value.metadata : undefined,
        compatibility: value.compatibility,
    };
}
export async function loadSkillEditor(client: NeoismClient, id: string, directory?: string): Promise<Editor> {
    const resource = await client.management.skills.get(id, { directory, scope: directory ? "workspace" : "installation" });
    const definition = record(resource.definition);
    const info = record(definition.info);
    const fallback: SkillWrite = {
        content: typeof definition.content === "string" ? definition.content : "",
        name: typeof info.name === "string" ? info.name : id,
        description: typeof info.description === "string" ? info.description : "",
        scope: resource.scope ?? undefined,
    };
    const base = { kind: "skills" as const, id, existing: true, revision: resource.revision ?? undefined, scope: resource.scope };
    if (!resource.writable || typeof definition.bundleReadOnlyReason === "string") return { ...base, definition: fallback, readOnly: true, reason: typeof definition.bundleReadOnlyReason === "string" ? definition.bundleReadOnlyReason : "This discovered or built-in skill is read-only." };
    if ("bundle" in definition || "bundleRevision" in definition) {
        const bundle = parseSkillBundle(definition.bundle);
        if (bundle && resource.revision && definition.bundleRevision === resource.revision && bundle.expectedRevision === resource.revision && bundle.scope === resource.scope) {
            return { ...base, definition: bundle };
        }
        // A new server attempted an authoritative read. Do not conceal a broken
        // or stale projection by replacing it with historical contents.
        return { ...base, definition: fallback, readOnly: true, reason: "The server returned an incomplete or mismatched skill bundle. Reload the skill before editing; support files have not been changed." };
    }
    try {
        const versions = await client.management.skills.versions(id, { directory, scope: resource.scope ?? undefined });
        const match = versions.find(v => v.skillId === id && v.scope === resource.scope && v.revision === resource.revision);
        if (match) {
            const version = await client.management.skills.version(id, match.id, { directory, scope: resource.scope ?? undefined });
            if (version.skillId === id && version.scope === resource.scope && version.revision === resource.revision) {
                return { ...base, definition: { ...version.bundle, scope: resource.scope ?? undefined, expectedRevision: resource.revision ?? undefined } };
            }
        }
    } catch { /* Never substitute an incomplete catalog projection for a bundle. */ }
    return { ...base, definition: fallback, readOnly: true, reason: "No complete saved bundle matches this skill’s current revision and scope. Saving would risk deleting support files. Upgrade the server to support complete skill bundle reads and reload, or edit the skill on disk." };
}
export function skillDeleteOptions(skill: Skill, directory?: string) {
    if (!skill.scope || !skill.revision) throw new Error("Cannot safely delete a skill without its scope and revision. Reload the library.");
    return { directory, scope: skill.scope, expectedRevision: skill.revision };
}
export async function saveSkillEditor(client: NeoismClient, editor: Extract<Editor, { kind: "skills" }>, directory?: string) {
    if (editor.readOnly) throw new Error(editor.reason || "Skill is read-only");
    if (editor.existing && (!editor.scope || !editor.revision)) throw new Error("Reload the skill to obtain its scope and revision.");
    const body: SkillWrite = { ...editor.definition, scope: editor.existing ? editor.scope ?? undefined : directory ? editor.definition.scope ?? "workspace" : "installation", expectedRevision: editor.existing ? editor.revision : undefined };
    return editor.existing ? client.management.skills.update(editor.id, body, { directory, expectedRevision: editor.revision }) : client.management.skills.create(editor.id, body, { directory });
}
export async function saveWorkflowEditor(plugin: WorkflowsClient, editor: Extract<Editor, { kind: "workflows" }>, directory?: string) {
    if (editor.readOnly) throw new Error("Workflow is read-only");
    if (editor.existing && !editor.revision) throw new Error("Reload the workflow to obtain its revision before editing.");
    const scope = editor.existing && editor.scope ? editor.scope : directory ? "workspace" as const : "installation" as const;
    return editor.existing
        ? plugin.update(editor.id, { ...editor.definition, id: editor.id }, { directory, scope, revision: editor.revision })
        : plugin.create(editor.definition, { directory, scope });
}
export function changeFrequency(schedule: FlowWrite["schedule"], frequency: string): FlowWrite["schedule"] {
    const base = { frequency, timezone: schedule.timezone, interval: frequency === "once" ? 1 : schedule.interval };
    switch (frequency) {
        case "hourly": return { ...base, minute: 0 };
        case "weekly": return { ...base, time: "09:00", weekdays: ["monday"] };
        case "monthly": return { ...base, time: "09:00", monthDay: 1 };
        case "once": return { ...base, date: new Date().toISOString().slice(0, 10), time: "09:00" };
        default: return { ...base, time: "09:00" };
    }
}
