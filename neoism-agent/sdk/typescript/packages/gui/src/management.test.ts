import { describe, it, expect } from "vitest";
import { createNeoismClient, createHttpTransport, workflows, type OperationResponse } from "@neoism/sdk";
import { loadSkillEditor, saveSkillEditor, saveWorkflowEditor, skillDeleteOptions, changeFrequency, type Skill } from "./management";
const resource: Skill = { id: "example", scope: "installation", revision: "current", writable: true, managed: true, origin: "managed", provenance: "/skills/example", createdAt: null, updatedAt: null, definition: { info: { id: "example", name: "Example", description: "Useful", path: "/skills/example/SKILL.md" }, content: "Instructions" } };
const version: OperationResponse<"v2.management.skills.versions.get"> = { id: "v1", skillId: "example", scope: "installation", revision: "current", createdAt: 1, bundle: { scope: "installation", content: "Instructions", name: "Example", description: "Useful", metadata: { custom: [1, 2] }, compatibility: { os: "linux" }, files: { "scripts/run.sh": "#!/bin/sh\necho hello", "references/help.md": "Help" } } };
function mock(versions = [version], current = resource) {
    const reads: string[] = [];
    const writes: { url: string; init?: RequestInit }[] = [];
    const client = createNeoismClient(createHttpTransport({ baseUrl: "http://localhost", fetch: async (input, init) => {
        const url = String(input);
        if (init?.method !== "GET") { writes.push({ url, init }); return init?.method === "DELETE" ? new Response(null, { status: 204 }) : Response.json(resource); }
        reads.push(url);
        return Response.json(new URL(url).pathname.endsWith("/versions/v1") ? version : url.includes("/versions") ? versions : current);
    } }));
    return { client, writes, reads };
}
describe("skill management actual wire projection", () => {
    it("reads info/content but writes the flattened complete bundle with immutable scope and revision", async () => {
        const { client, writes } = mock();
        const editor = await loadSkillEditor(client, "example", "/workspace");
        if (editor.kind !== "skills") throw Error("wrong editor");
        editor.definition.content = "Edited";
        editor.definition.scope = "workspace";
        await saveSkillEditor(client, editor, "/workspace");
        expect(writes).toHaveLength(1);
        const body: unknown = JSON.parse(String(writes[0].init?.body));
        expect(body).toEqual({ ...version.bundle, content: "Edited", scope: "installation", expectedRevision: "current" });
        expect(writes[0].url).toContain("directory=%2Fworkspace");
        expect(new Headers(writes[0].init?.headers).get("if-match")).toBe("current");
    });
    it.each([[], [{ ...version, revision: "old" }], [{ ...version, scope: "workspace" as const }]])("refuses incomplete, stale or wrong-scope bundles", async (...items) => {
        const { client, writes } = mock(items);
        const editor = await loadSkillEditor(client, "example", "/workspace");
        expect(editor.readOnly).toBe(true);
        expect(editor.reason).toContain("support files");
        if (editor.kind !== "skills") throw Error("wrong editor");
        await expect(saveSkillEditor(client, editor, "/workspace")).rejects.toThrow();
        expect(writes).toHaveLength(0);
    });
    it("deletes with outer scope and revision", async () => {
        const { client, writes } = mock();
        await client.management.skills.delete(resource.id, skillDeleteOptions(resource, "/workspace"));
        expect(writes[0].url).toContain("scope=installation");
        expect(new Headers(writes[0].init?.headers).get("if-match")).toBe("current");
    });
});
describe("frequency-specific schedules", () => {
    const old = { frequency: "once", interval: 7, timezone: "UTC", at: "2027-01-01T00:00:00Z", date: "2027-01-01", time: "12:00", weekdays: ["friday"], monthDay: 3, minute: 20 };
    it.each(["hourly", "daily", "weekly", "monthly", "once"])("clears incompatible fields for %s", frequency => {
        const next = changeFrequency(old, frequency);
        expect(next.timezone).toBe("UTC");
        expect(next.at).toBeUndefined();
        expect(next.minute !== undefined).toBe(frequency === "hourly");
        expect(next.weekdays !== undefined).toBe(frequency === "weekly");
        expect(next.monthDay !== undefined).toBe(frequency === "monthly");
        expect(next.date !== undefined).toBe(frequency === "once");
        expect(next.time !== undefined).toBe(frequency !== "hourly");
        if (frequency === "once") expect(next.interval).toBe(1);
    });
});

describe("workflow revision updates", () => {
    it("retains all settings and sends the outer revision and immutable ID", async () => {
        const { client, writes } = mock();
        const plugin = workflows.client(client);
        const definition = { id: "attempted-rename", name: "Flow", active: false, prompt: "Run", schedule: changeFrequency({ frequency: "daily", interval: 1, timezone: "UTC" }, "weekly"), retry: { maxAttempts: 3 }, permissions: { bash: "ask" }, model: { providerId: "openai", id: "model" } };
        await saveWorkflowEditor(plugin, { kind: "workflows", existing: true, id: "flow", revision: "outer-revision", definition }, "/workspace");
        expect(JSON.parse(String(writes[0].init?.body))).toEqual({ ...definition, id: "flow" });
        expect(new Headers(writes[0].init?.headers).get("if-match")).toBe("outer-revision");
    });
});

describe("global resource requests", () => {
    it("creates a global skill with no directory, even if a draft tries workspace scope", async () => {
        const { client, writes } = mock();
        await saveSkillEditor(client, { kind: "skills", existing: false, id: "example", definition: { scope: "workspace", content: "Instructions", files: { "help.md": "Keep" } } });
        expect(JSON.parse(String(writes[0].init?.body))).toMatchObject({ scope: "installation", files: { "help.md": "Keep" } });
        expect(new URL(writes[0].url).searchParams.has("directory")).toBe(false);
    });
    it("loads complete global bundles and scopes all version reads, restore and delete", async () => {
        const { client, reads, writes } = mock();
        const editor = await loadSkillEditor(client, "example");
        expect(editor.readOnly).not.toBe(true);
        for (const request of reads) {
            expect(new URL(request).searchParams.get("scope")).toBe("installation");
            expect(new URL(request).searchParams.has("directory")).toBe(false);
        }
        await client.management.skills.restore("example", "v1", { scope: "installation", expectedRevision: "current" });
        await client.management.skills.delete("example", skillDeleteOptions(resource));
        for (const request of writes) expect(new URL(request.url).searchParams.get("scope")).toBe("installation");
    });
    it("keeps workflow execution directory separate from global definition scope", async () => {
        const { client, writes, reads } = mock();
        const plugin = workflows.client(client);
        const definition = { id: "example", name: "Example", prompt: "Instructions", active: false, directory: "/execution-only", schedule: { frequency: "daily", time: "09:00", interval: 1, timezone: "UTC" } };
        await saveWorkflowEditor(plugin, { kind: "workflows", existing: false, id: "example", definition });
        await saveWorkflowEditor(plugin, { kind: "workflows", existing: true, id: "example", revision: "current", definition });
        await plugin.activate("example", { scope: "installation" });
        await plugin.pause("example", { scope: "installation" });
        await plugin.history("example", { scope: "installation" });
        await plugin.preview("example", { scope: "installation" });
        await plugin.remove("example", { scope: "installation", revision: "current" });
        expect(JSON.parse(String(writes[0].init?.body)).directory).toBe("/execution-only");
        for (const request of [...writes.map(item => item.url), ...reads]) {
            expect(new URL(request).searchParams.get("scope")).toBe("installation");
            expect(new URL(request).searchParams.has("directory")).toBe(false);
        }
    });
});

const diskBundle = { ...version.bundle, content: "Current disk instructions", expectedRevision: "disk-revision", name: null, description: null, version: null, license: null };
function diskResource(bundle: unknown = diskBundle, bundleRevision = "disk-revision"): Skill {
    return { ...resource, revision: "disk-revision", definition: { info: { id: "example", name: "Example" }, content: "Current disk instructions", bundle, bundleRevision } };
}
describe("complete GET bundle projection", () => {
    it("edits a non-versioned disk skill without requesting historical versions", async () => {
        const { client, writes, reads } = mock([], diskResource());
        const editor = await loadSkillEditor(client, "example", "/workspace");
        if (editor.kind !== "skills") throw Error("wrong editor");
        expect(editor.readOnly).not.toBe(true);
        expect(editor.definition.content).toBe("Current disk instructions");
        editor.definition.content = "Edited disk skill";
        await saveSkillEditor(client, editor, "/workspace");
        expect(reads).toHaveLength(1);
        expect(reads[0]).not.toContain("versions");
        expect(JSON.parse(String(writes[0].init?.body))).toEqual({
            scope: "installation", content: "Edited disk skill", expectedRevision: "disk-revision",
            files: version.bundle.files, metadata: version.bundle.metadata, compatibility: version.bundle.compatibility,
        });
        expect(new Headers(writes[0].init?.headers).get("if-match")).toBe("disk-revision");
    });
    it.each([
        diskResource({ ...diskBundle, files: undefined }),
        diskResource({ ...diskBundle, files: { "binary.dat": [255] } }),
        diskResource({ ...diskBundle, expectedRevision: "stale" }),
        diskResource({ ...diskBundle, scope: "workspace" }),
        diskResource(diskBundle, "stale"),
        diskResource({ ...diskBundle, unknownField: "do not drop" }),
    ])("refuses malformed or mismatched authoritative projections without historical fallback", async current => {
        const { client, writes, reads } = mock([version], current);
        const editor = await loadSkillEditor(client, "example", "/workspace");
        expect(editor.readOnly).toBe(true);
        expect(reads).toHaveLength(1);
        if (editor.kind !== "skills") throw Error("wrong editor");
        await expect(saveSkillEditor(client, editor, "/workspace")).rejects.toThrow();
        expect(writes).toHaveLength(0);
    });
    it("shows backend unreadable/binary bundle explanation instead of attempting a partial save", async () => {
        const reason = "Cannot safely edit: binary support file. Check the skill files on disk and reload.";
        const { client, reads } = mock([version], { ...resource, writable: false, definition: { info: { name: "Example" }, content: "Instructions", bundleReadOnlyReason: reason } });
        const editor = await loadSkillEditor(client, "example", "/workspace");
        expect(editor.reason).toBe(reason);
        expect(editor.readOnly).toBe(true);
        expect(reads).toHaveLength(1);
    });
});
