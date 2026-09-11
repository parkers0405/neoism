// @vitest-environment happy-dom
import { act, useState, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { createHttpClient } from "@neoism/sdk";
import { SkillDefinitionEditor, renameSupportFile, skillValidation } from "./SkillDefinitionEditor";
import { StructuredValueEditor } from "./StructuredValueEditor";
import { ResourceEditorDialog } from "./ResourceEditorDialog";
import { Library } from "./Library";
import type { Editor } from "../management";
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root | undefined;
let host: HTMLDivElement;
async function render(node: ReactNode) { host = document.createElement("div"); document.body.append(host); root = createRoot(host); await act(async () => root!.render(node)); }
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; host?.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function click(text: string) { const button = [...host.querySelectorAll("button")].find(b => b.textContent === text); expect(button).toBeTruthy(); await act(async () => button!.click()); }
async function input(selector: string, value: string) { const element = host.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)!; expect(element).toBeTruthy(); await act(async () => { Object.getOwnPropertyDescriptor(element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, "value")!.set!.call(element, value); element.dispatchEvent(new Event("input", { bubbles: true })); }); }
const initial: Extract<Editor, {kind: "skills"}> = { kind: "skills", id: "review", existing: true, scope: "installation", revision: "r1", definition: { name: "Review", description: "Review code", scope: "installation", content: "# Instructions", files: { "a.txt": "keep" }, metadata: { enabled: false, count: 0, empty: "", absent: null, list: [0, false] }, compatibility: [false, null, 0] } };
describe("complete skill resource form", () => {
    it("adds, renames and deletes support files without changing typed metadata", async () => {
        let latest: Extract<Editor, {kind: "skills"}> = initial;
        function Form() { const [editor, setEditor] = useState<Editor>(initial); if (editor.kind === "skills") latest = editor; return editor.kind === "skills" && <SkillDefinitionEditor editor={editor} change={setEditor} />; }
        await render(<Form />);
        await input('[aria-label="New support file path"]', "b.txt"); await click("+ Add file");
        expect(latest.definition.files).toEqual({ "a.txt": "keep", "b.txt": "" });
        await input('.support-file input', "renamed.txt"); await click("Rename");
        expect(latest.definition.files).toEqual({ "renamed.txt": "keep", "b.txt": "" });
        await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Remove b.txt"]')!.click());
        expect(latest.definition.files).toEqual({ "renamed.txt": "keep" });
        expect(latest.definition.metadata).toEqual(initial.definition.metadata);
        expect(latest.definition.compatibility).toEqual([false, null, 0]);
        expect(host.querySelector<HTMLInputElement>('[placeholder="code-review"]')!.readOnly).toBe(true);
        expect(host.textContent).toContain("Global skill · scope cannot be changed");
    });
    it("edits and removes falsy structured values without stringifying the object", async () => {
        let latest: unknown;
        function Form() { const [value, setValue] = useState<unknown>({ no: false, zero: 0, empty: "", nil: null, list: [0, 2] }); latest = value; return <StructuredValueEditor label="Metadata" objectOnly value={value} onChange={setValue} />; }
        await render(<Form />);
        expect(latest).toEqual({ no: false, zero: 0, empty: "", nil: null, list: [0, 2] });
        await input('[aria-label="zero"]', "7");
        await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Remove nil"]')!.click());
        await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Remove list item 1"]')!.click());
        expect(latest).toEqual({ no: false, zero: 7, empty: "", list: [2] });
        expect(host.querySelector<HTMLInputElement>('[aria-label="list item 1"]')!.value).toBe("2");
    });
    it("rejects unsafe paths, collisions and bundle size violations", () => {
        for (const path of ["../bad", "/absolute", "a/./b", "SKILL.md", "a\\b"]) expect(() => renameSupportFile({ "a.txt": "x" }, "a.txt", path)).toThrow();
        expect(() => renameSupportFile({ a: "x", b: "y" }, "a", "b")).toThrow();
        expect(skillValidation({ ...initial, id: "Bad ID" }).id).toBeTruthy();
        expect(skillValidation({ ...initial, definition: { ...initial.definition, content: "a".repeat(512 * 1024 + 1) } }).content).toBeTruthy();
        expect(skillValidation({ ...initial, definition: { ...initial.definition, files: { a: "a".repeat(256 * 1024 + 1) } } }).files).toBeTruthy();
    });
    it("uses a dedicated sticky shell with contextual actions and captured destination", () => {
        const shell = (readOnly = false, existing = false) => renderToStaticMarkup(<ResourceEditorDialog kind="skills" directory="/selected/project" readOnly={readOnly} existing={existing} busy={false} close={() => {}} submit={() => {}}>Fields</ResourceEditorDialog>);
        expect(shell()).toContain("Create skill"); expect(shell(false, true)).toContain("Save changes");
        expect(shell(true, true)).not.toContain('type="submit"'); expect(shell()).toContain("/selected/project");
        const css = readFileSync("src/resource-editor.css", "utf8"); expect(css).toContain("position: sticky"); expect(css).toContain("var(--bg)"); expect(css).toContain("24px");
    });
    it("gates creation but keeps read-only details viewable", async () => {
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async request => {
            const url = new URL(String(request));
            if (url.pathname === "/v2/directories") return Response.json({ path: "/canonical/project", entries: [] });
            if (url.pathname === "/v2/capabilities") return Response.json([{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: false }]);
            const skill = { id: "review", kind: "skill", writable: false, origin: "builtin", scope: "installation", revision: "r1", definition: { info: { name: "Review", description: "Read only" }, content: "Read these instructions" } };
            return Response.json(url.pathname.endsWith("/review") ? skill : [skill]);
        } });
        await render(<Library kind="skills" directory="relative" client={client} openSession={() => {}} />);
        expect([...host.querySelectorAll("button")].find(b => b.textContent?.includes("New skill"))!.disabled).toBe(true);
        await click("View details"); expect(host.textContent).toContain("Read these instructions");
        expect(host.querySelector('button[type="submit"]')).toBeNull();
        expect(host.textContent).toContain("Global skills");
    });
    it("round-trips an edited complete bundle to the canonical project and preserves the draft after authorization denial", async () => {
        let writes: unknown[] = [], listReads = 0, reject = true;
        const destinations: string[] = [];
        const resource = { id: "review", kind: "skill", writable: true, origin: "managed", scope: "installation", revision: "r1", definition: { bundleRevision: "r1", bundle: { ...initial.definition, expectedRevision: "r1" } } };
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async (request, init) => {
            const url = new URL(String(request));
            if (url.pathname === "/v2/directories") return Response.json({ path: "/canonical/project", entries: [] });
            if (url.pathname === "/v2/capabilities") return Response.json([{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: true }]);
            if (init?.method === "PUT" || init?.method === "PATCH") {
                writes.push(JSON.parse(String(init.body))); destinations.push(url.searchParams.get("directory") ?? "");
                return reject ? Response.json({ message: "Operator access required" }, { status: 403 }) : Response.json(resource);
            }
            if (url.pathname.endsWith("/review")) return Response.json(resource);
            listReads++; return Response.json([resource]);
        } });
        await render(<Library kind="skills" directory="relative" client={client} openSession={() => {}} />);
        await click("Edit");
        await input('[aria-label="New support file path"]', "new.txt"); await click("+ Add file");
        await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
        expect(writes).toHaveLength(1);
        expect(host.textContent).toContain("server denied operator authorization");
        expect(host.querySelector('button[type="submit"]')).toBeNull();
        expect(host.textContent).toContain("new.txt");
        reject = false; await click("Retry authorization");
        await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
        expect(writes).toHaveLength(2);
        expect(writes[1]).toMatchObject({ metadata: initial.definition.metadata, compatibility: [false, null, 0], expectedRevision: "r1", scope: "installation", files: { "a.txt": "keep", "new.txt": "" } });
        expect(destinations).toEqual(["", ""]);
        expect(listReads).toBeGreaterThanOrEqual(3);
        expect(host.querySelector("dialog")).toBeNull();
    });

    it("loads complete historical bundles and restores only after confirmation with current revision", async () => {
        let restored = false;
        const calls: URL[] = [];
        const old = { id: "v0", skillId: "review", scope: "installation", revision: "r0", createdAt: 1700000000000, bundle: { ...initial.definition, content: "Historical instructions", files: { "old.txt": "old contents" } } };
        const current = () => ({ id: "review", kind: "skill", writable: true, origin: "managed", scope: "installation", revision: restored ? "r2" : "r1", definition: { bundleRevision: restored ? "r2" : "r1", bundle: { ...(restored ? old.bundle : initial.definition), expectedRevision: restored ? "r2" : "r1" } } });
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async request => {
            const url = new URL(String(request)); calls.push(url);
            if (url.pathname === "/v2/directories") return Response.json({ path: "/canonical/project", entries: [] });
            if (url.pathname === "/v2/capabilities") return Response.json([{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: true }]);
            if (url.pathname.endsWith("/restore")) { restored = true; return Response.json(current()); }
            if (url.pathname.endsWith("/versions/v0")) return Response.json(old);
            if (url.pathname.endsWith("/versions")) return Response.json([old]);
            return Response.json(url.pathname.endsWith("/review") ? current() : [current()]);
        } });
        const confirmation = vi.fn(() => false); vi.stubGlobal("confirm", confirmation);
        await render(<Library kind="skills" directory="relative" client={client} openSession={() => {}} />);
        await click("Edit"); await click("Load versions"); await click("View version");
        expect(host.textContent).toContain("Historical instructions"); expect(host.textContent).toContain("old contents");
        await click("Restore"); expect(restored).toBe(false);
        confirmation.mockReturnValue(true); await click("Restore");
        expect(restored).toBe(true);
        const restore = calls.find(url => url.pathname.endsWith("/restore"))!;
        expect(restore.searchParams.get("expectedRevision")).toBe("r1");
        expect(restore.searchParams.has("directory")).toBe(false);
        expect(restore.searchParams.get("scope")).toBe("installation");
        expect(host.querySelector<HTMLTextAreaElement>('.skill-definition .resource-code[rows="14"]')!.value).toBe("Historical instructions");
    });

    it("counts decoded UTF-8 content and files, not JSON escaping or frontmatter", () => {
        const validate = (content: string, files: Record<string, string>) => skillValidation({ ...initial, definition: { ...initial.definition, content, files, metadata: { note: "x".repeat(1024 * 1024) } } });
        const escaped = '\n"\\\n'.repeat(200 * 1024 / 4);
        // 300 KiB instructions + three 200 KiB files: transport escaping exceeds 1 MiB.
        expect(new TextEncoder().encode(escaped).length).toBe(200 * 1024);
        expect(validate("x".repeat(300 * 1024), { a: escaped, b: escaped, c: escaped })).toEqual({});
        const unicodeFile = "😀".repeat(64 * 1024); // exactly 256 KiB, not 128 KiB
        const unicodeInstructions = "é".repeat(256 * 1024); // exactly 512 KiB
        expect(validate(unicodeInstructions, { a: unicodeFile, b: unicodeFile })).toEqual({});
        expect(validate(unicodeInstructions + "é", {})).toHaveProperty("content");
        expect(validate("Instructions", { a: unicodeFile + "😀" }).files).toContain("256 KiB");
        expect(validate(unicodeInstructions, { a: unicodeFile, b: unicodeFile, c: "x" }).files).toContain("1 MiB");
        expect(validate("Instructions", Object.fromEntries(Array.from({ length: 33 }, (_, i) => [`file-${i}`, ""])) ).files).toContain("32");
    });
    it.each([false, true])("closes a committed %s-existing save before failed list refresh and adopts its revision", async existing => {
        let committed = false, allowRefresh = false;
        const writes: string[] = [], deleteRevisions: Array<string | null> = [];
        let listReads = 0;
        const resource = () => ({ id: "review", kind: "skill", writable: true, origin: "managed", scope: "installation", revision: committed ? "r2" : "r1", definition: { bundleRevision: committed ? "r2" : "r1", bundle: { ...initial.definition, expectedRevision: committed ? "r2" : "r1" } } });
        const client = createHttpClient({ baseUrl: "http://agent.test", fetch: async (request, init) => {
            const url = new URL(String(request));
            if (url.pathname === "/v2/directories") return Response.json({ path: "/canonical/project", entries: [] });
            if (url.pathname === "/v2/capabilities") return Response.json([{ id: "neoism.resources.installation", enabled: true }, { id: "neoism.management", enabled: true }]);
            if (init?.method === "POST" || init?.method === "PUT") { writes.push(init.method); committed = true; return Response.json(resource()); }
            if (init?.method === "DELETE") { deleteRevisions.push(new Headers(init.headers).get("If-Match")); return new Response(null, { status: 204 }); }
            if (url.pathname.endsWith("/review")) return Response.json(resource());
            listReads++;
            if (committed && !allowRefresh) return Response.json({ message: "List unavailable" }, { status: 503 });
            return Response.json(existing || committed ? [resource()] : []);
        } });
        await render(<Library kind="skills" directory="relative" client={client} openSession={() => {}} />);
        if (existing) await click("Edit");
        else {
            await click("+ New skill");
            await input('[placeholder="code-review"]', "review");
            await input('.skill-definition .form-field input', "Review");
        }
        await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
        expect(writes).toEqual([existing ? "PUT" : "POST"]);
        expect(host.querySelector("dialog")).toBeNull();
        expect(host.textContent).toContain("Saved, but the list could not refresh.");
        expect(host.querySelector(".resource h2")!.textContent).toBe("review");
        await click("Retry refresh"); // still fails; must not retry the write or lose the committed card
        expect(listReads).toBe(3);
        expect(writes).toHaveLength(1);
        expect(host.querySelector(".resource h2")!.textContent).toBe("review");
        if (existing) {
            vi.stubGlobal("confirm", () => true);
            await click("Delete");
            // The card already owns r2 from the mutation response, before any successful list GET.
            expect(deleteRevisions[0]).toBe("r2");
        }
        allowRefresh = true;
        await click("Retry refresh");
        expect(writes).toHaveLength(1);
        expect(host.textContent).not.toContain("Saved, but the list could not refresh.");
    });

});
