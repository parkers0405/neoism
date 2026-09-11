// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ToolPart } from "./ToolPart";
import { readFileSync } from "node:fs";
const toolStyles = readFileSync("src/components/tool-cards.css", "utf8");
import { fileChanges, parsePatch, prettyPreview, taskIdentity, type CardPart } from "./toolCardData";
const tool = (name: string, state: Record<string, unknown>): CardPart => ({ id: "p", messageId: "m", sessionId: "parent", callId: "c", type: "tool", tool: name, state } as CardPart);
// Shapes emitted by snapshot.rs::add_metadata_snapshots/file_patch_metadata and tool_tests.rs.
const rustPatch = { status: "completed", input: { patchText: "*** Begin Patch\n*** Update File: TASK.md\n@@\n-before\n+after\n+again\n*** End Patch" }, metadata: {
    files: [{ relativePath: "TASK.md", filePath: "TASK.md", type: "update", additions: 999, patch: "--- a/TASK.md\n+++ b/TASK.md\n@@ -1,3 +1,4 @@\n one\n-before\n+after\n+again\n three" }],
    diffs: [{ path: "TASK.md", kind: "modified", additions: 999, deletions: 1, beforeSha256: "hash" }],
}, output: "Success. Updated TASK.md", time: { start: 1000, end: 2000 } };
let root: Root, el: HTMLDivElement;
beforeEach(() => { (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true; el = document.createElement("div"); document.body.append(el); root = createRoot(el); });
afterEach(() => { act(() => root.unmount()); el.remove(); });
it("keeps raw edit arguments and cached diagnostics out of the diff view", () => {
    const part = tool("edit", {status:"completed",input:{filePath:"a.ts",oldString:"before",newString:"after",replaceAll:false},
        output:"Replaced 1 occurrence(s)",metadata:{diagnostics:[{message:"STALE_DIAGNOSTIC",freshness:"unknown"}]}});
    act(() => root.render(<ToolPart part={part} />));
    expect(el.querySelector(".tc-compact-diff")?.textContent).toContain("after");
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.querySelector(".tc-line-add")?.textContent).toContain("after");
    expect(el.textContent).not.toMatch(/Replaced 1|replaceAll|STALE_DIAGNOSTIC|Arguments|Tool details|Browse all fields/);
    expect(el.querySelectorAll("summary")).toHaveLength(0);
});
it("uses pending edit copy before patch arguments arrive", () => {
    act(() => root.render(<ToolPart part={tool("apply_patch",{status:"pending",input:{}})} />));
    expect(el.textContent).toContain("Pending edit");
    expect(el.textContent).not.toContain("No diff supplied");
});
it("leaves successful todo data to the dedicated checklist and still exposes failures", () => {
    act(() => root.render(<ToolPart part={tool("todowrite",{status:"completed",input:{todos:[]},output:"[]"})} />));
    expect(el.textContent).toBe("");
    act(() => root.render(<ToolPart part={tool("todowrite",{status:"error",error:"Cannot save todos"})} />));
    expect(el.textContent).toContain("Cannot save todos");
});
it("uses Rust patch metadata for a compact preview and expandable parsed diff", () => {
    const part = tool("apply_patch", rustPatch), changes = fileChanges(part);
    expect(changes).toHaveLength(1); expect(changes[0].source).toBe("result");
    act(() => root.render(<ToolPart part={part} />));
    expect(el.querySelector(".tc-diff")).toBeNull();
    act(() => el.querySelector<HTMLButtonElement>(".tc-compact-diff")!.click());
    expect(el.textContent).toContain("+2"); expect(el.textContent).not.toContain("999");
    expect(el.querySelectorAll(".tc-line-add")).toHaveLength(2); expect(el.querySelectorAll(".tc-line-remove")).toHaveLength(1);
});
it("supports replacement list inputs, snapshots and nested output without inventing write diffs", () => {
    expect(fileChanges(tool("replace_text", { input: [{ filePath: "a.rs", oldString: "before", newString: "after" }, { path: "b.rs", oldText: "a", newText: "b" }] }))).toHaveLength(2);
    expect(fileChanges(tool("edit", { metadata: { snapshots: [{ path: "a", before: { exists: true, contentBase64: btoa("old") }, after: { exists: true, contentBase64: btoa("new") } }] } }))[0].rows.map(r => r.text)).toEqual(["old", "new"]);
    expect(fileChanges(tool("edit", { output: JSON.stringify({ diffs: [{ path: "x", before: "old", after: "new" }] }) }))[0].path).toBe("x");
    for (const state of [{ input: { filePath: "x", content: "new" } }, { metadata: { diffs: [{ path: "x", additions: 10 }] } }]) expect(fileChanges(tool("write", state))).toEqual([]);
    expect(parsePatch("*** Begin Patch\n*** Delete File: x\n*** End Patch")[0].rows).toEqual([]);
});
it("preserves exact tool statuses and semantic errors, escaping paths and terminal controls", () => {
    for (const status of ["pending", "running", "completed", "error"]) {
        const html = renderToStaticMarkup(<ToolPart part={tool("mcp:server.apply_patch", { ...rustPatch, status, error: status === "error" ? "\x1b[31mPermission denied\x1b[0m" : undefined, metadata: { files: [{ filePath: '<img onerror="x">', patch: "@@ -1 +1 @@\n-old\n+new" }] } })} />);
        expect(html).toContain(`data-tool-status="${status}"`); expect(html).not.toContain("<img"); expect(html).not.toContain("\x1b");
        if (status === "error") expect(html).toContain("Permission denied");
    }
});
it("keeps Rust task tool status distinct from child lifecycle; navigates only to child", () => {
    const onOpenSession = vi.fn(), part = tool("task", { status: "completed", input: { description: "Investigate", subagent_type: "explore" }, metadata: { sessionId: "ses_child", agent: "explore", status: "running", background: true }, output: "task_id: ses_child (use this to check or continue the subagent task)" });
    act(() => root.render(<ToolPart part={part} onOpenSession={onOpenSession} />));
    expect(el.querySelector('[data-tool-status="completed"]')).not.toBeNull(); expect(el.querySelector('[data-task-status="running"]')).not.toBeNull(); expect(el.textContent).not.toContain("task_id:"); expect(el.textContent).not.toContain("Stop task");
    act(() => ([...el.querySelectorAll("button")].find(b => b.textContent?.includes("Open session"))!).click()); expect(onOpenSession).toHaveBeenCalledWith("ses_child");
    expect(taskIdentity(tool("task", { metadata: { taskId: "runtime_id" } }))).toMatchObject({ taskId: "runtime_id", sessionId: "" });
    for (const key of ["sessionID", "sessionId", "sessID"]) expect(taskIdentity(tool("task", { metadata: { [key]: "child" } })).sessionId).toBe("child");
});
it("shows stop only with callback and actual task ID and surfaces callback failures", async () => {
    const stop = vi.fn().mockRejectedValue(new Error("Denied")), part = tool("task", { status: "running", metadata: { taskId: "runtime_id" } });
    act(() => root.render(<ToolPart part={part} onStopTask={stop} />));
    await act(async () => ([...el.querySelectorAll("button")].find(b => b.textContent === "Stop task"))!.click());
    expect(stop).toHaveBeenCalledWith("runtime_id"); expect(el.textContent).toContain("Denied");
});
it("handles subtask instructions and MCP generic structured details lazily with bounded output", () => {
    const part = tool("mcp:custom.operation", { status: "completed", input: ["keep", { hidden: "VALUE" }], output: "x".repeat(2_000_000) + "TAIL" });
    act(() => root.render(<ToolPart part={part} />)); expect(el.innerHTML.length).toBeLessThan(6000); expect(el.textContent).not.toContain("VALUE");
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click()); expect(el.textContent).not.toContain("VALUE"); expect(el.querySelectorAll("summary")).toHaveLength(0); expect(el.innerHTML.length).toBeLessThan(20000); expect(el.textContent).not.toContain("TAIL");
    expect(prettyPreview({ text: "x".repeat(2_000_000) }).length).toBeLessThan(2600);
    act(() => root.render(<ToolPart part={{ type: "subtask", id: "sub", sessionId: "s", messageId: "m", agent: "explore", description: "Inspect", prompt: "PRIVATE" }} />));
    expect(el.textContent).toContain("Inspect"); expect(el.querySelector('[data-task-status="requested"]')).not.toBeNull(); expect(el.textContent).not.toContain("PRIVATE");
});
it("caps huge diff rendering and suppresses misleading partial counts", () => {
    const part = tool("apply_patch", { status: "completed", input: { patchText: "*** Begin Patch\n*** Add File: big\n" + "+line\n".repeat(30000) + "*** End Patch" } });
    act(() => root.render(<ToolPart part={part} />));
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.querySelector(".tc-counts")).toBeNull();
    expect(el.querySelectorAll(".tc-diff-row").length).toBeLessThanOrEqual(40); expect(el.textContent).toContain("shortened diff preview");
});
it("renders pending raw arguments on each full-part stream update, including unfinished JSON escapes", () => {
    // provider_stream_processor ToolInputStart => pending input:{}; Delta => accumulated raw.
    const patch = '*** Begin Patch\n*** Update File: src/lib.rs\n*** Move to: src/main.rs\n@@ fn main\n-old();\n+  println!("quoted \\\"value\\\"");\n*** Add File: new.rs\n+// keep \\ path\n*** Delete File: old.rs\n*** End Patch';
    const raw = JSON.stringify({ description: 'comment with "patchText": "not an argument"', patchText: patch });
    const end = raw.indexOf('println!') + 8;
    act(() => root.render(<ToolPart part={tool("functions.apply_patch", { status: "pending", input: {}, raw: raw.slice(0, end) })} />));
    expect(el.querySelector('[data-tool-status="pending"]')).not.toBeNull();
    expect(el.textContent).toContain("src/lib.rs"); expect(el.textContent).toContain("src/main.rs"); expect(el.querySelector(".tc-line-add")?.textContent).toContain("println");
    expect(el.querySelector(".tc-counts")).toBeNull(); // do not count an unfinished streamed line
    for (const stop of [end + 2, raw.length - 1, raw.length]) {
        act(() => root.render(<ToolPart part={tool("functions.apply_patch", { status: "pending", input: {}, raw: raw.slice(0, stop) })} />));
        expect(el.textContent).not.toContain("not an argument");
    }
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.querySelectorAll(".tc-file")).toHaveLength(3); expect(el.querySelector(".tc-line-hunk")?.textContent).toContain("@@ fn main");
    expect(el.textContent).toContain("File deletion requested");
    expect(el.querySelectorAll(".tc-line-add")[0].querySelector("code")?.textContent).toBe('  println!("quoted \\\"value\\\"");');
    act(() => root.render(<ToolPart part={tool("functions.apply_patch", { status: "running", input: { patchText: patch }, time: { start: 10 } })} />));
    expect(el.querySelector('[data-tool-status="running"]')).not.toBeNull(); expect(el.querySelectorAll(".tc-file")).toHaveLength(3);
});
it("accepts freeform, aliases, partialRaw and completed metadata JSON but never write content as a diff", () => {
    const patch = "*** Begin Patch\n*** Add File: a.rs\n+let a = 1;\n*** End Patch";
    for (const state of [
        { status: "pending", input: {}, raw: patch },
        { status: "running", input: {}, partialRaw: JSON.stringify({ content: patch }).slice(0, -1) },
        { status: "running", input: { patch } },
        { status: "running", input: patch },
        { status: "completed", input: {}, metadata: JSON.stringify({ files: [{ filePath: "a.rs", patch }] }) },
    ]) expect(fileChanges(tool("functions.apply_patch", state))[0].path).toBe("a.rs");
    expect(fileChanges(tool("write", { input: { filePath: "doc.txt", content: patch } }))).toEqual([]);
    expect(fileChanges(tool("bash", { input: { content: patch } }))).toEqual([]);
    expect(fileChanges(tool("apply_patch", { input: { content: "normal output with @@ mention" } }))).toEqual([]);
    const partial = '{"patchText":"*** Begin Patch\\n*** Add File: u\\u002e';
    expect(fileChanges(tool("apply_patch", { status: "pending", input: {}, raw: partial }))[0].path).toBe("u.");
    expect(fileChanges(tool("apply_patch", { status: "pending", input: {}, raw: partial.slice(0, -1) }))[0].path).toBe("u");
});
it("splits header-only unified multi-file patches and does not confuse deleted header-like code with file headers", () => {
    const patch = "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n--- old comment\n+++ new comment\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old\n+new\n";
    const changes = parsePatch(patch);
    expect(changes.map(c => c.path)).toEqual(["a.rs", "b.rs"]);
    expect(changes[0].rows[1]).toMatchObject({ kind: "remove", text: "-- old comment" });
    expect(changes[0].rows[2]).toMatchObject({ kind: "add", text: "++ new comment" });
});
it("shows a small diff first, then exposes the complete scrollable diff without paging buttons", async () => {
    const copy = vi.fn().mockResolvedValue(undefined); vi.spyOn(navigator.clipboard, "writeText").mockImplementation(copy);
    const patch = "*** Begin Patch\n*** Add File: a\n" + Array.from({ length: 160 }, (_, i) => `+line ${i}\n`).join("") + "*** End Patch";
    const part = tool("apply_patch", { status: "running", input: { patchText: patch } });
    act(() => root.render(<ToolPart part={part} />));
    expect(el.querySelectorAll(".tc-compact-diff .tc-diff-row")).toHaveLength(6);
    act(() => el.querySelector<HTMLButtonElement>(".tc-compact-diff")!.click());
    expect(el.querySelectorAll(".tc-diff-row").length).toBeLessThanOrEqual(40);
    expect(el.textContent).not.toMatch(/Show more lines|Previous lines|Next lines/);
    const viewport = el.querySelector<HTMLElement>(".tc-diff")!;
    act(() => { viewport.scrollTop = 2480; viewport.dispatchEvent(new Event("scroll")); });
    expect(viewport.textContent).toContain("line 159");
    await act(async () => ([...el.querySelectorAll("button")].find(button => button.textContent === "Copy diff"))!.click());
    expect(copy).toHaveBeenCalledWith(expect.stringContaining("+line 159\n"));
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    act(() => root.render(<ToolPart part={tool("apply_patch", { status: "completed", input: { patchText: patch } })} />));
    expect(el.querySelector(".tc-diff")).toBeNull();
    expect(el.querySelector(".tc-compact-diff")).not.toBeNull();
    vi.restoreAllMocks();
});
it("Task is one clickable row plus direct session action, with only real output behind it", () => {
    const description = "Use compact expandable tool trees", navigate = vi.fn();
    const part = tool("task", { status: "completed", input: { description, subagent_type: "general" }, metadata: { sessionId: "ses_child", status: "running" }, output: "<unsafe>actual result</unsafe>" });
    act(() => root.render(<ToolPart part={part} onOpenSession={navigate} childStatus="outstanding" />));
    expect(el.textContent).toBe(`Task(${description})Open session ↗╰─ Waiting for task output…`);
    expect(toolStyles).toContain("animation: tc-square-orbit 1.2s linear infinite");
    expect(toolStyles).toContain("@media (prefers-reduced-motion: reduce) { .tc-task-orbit-dot { animation: none; } }");
    expect(el.querySelectorAll(".tc-task-orbit circle")).toHaveLength(4);
    expect(el.querySelector(".tc-task-orbit path, .tc-task-orbit rect")).toBeNull();
    expect(el.querySelector(".tc-task-row .tc-chevron")).toBeNull();
    expect(el.querySelectorAll("summary")).toHaveLength(0);
    act(() => el.querySelector<HTMLButtonElement>(".tc-open-session")!.click());
    expect(navigate).toHaveBeenCalledWith("ses_child"); expect(el.querySelector(".tc-tool-toggle")?.getAttribute("aria-expanded")).toBe("false");
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.textContent).toContain("Waiting for task output");
    act(() => root.render(<ToolPart part={part} onOpenSession={navigate} childStatus="completed" />));
    expect(el.querySelector(".tc-tree-body pre")?.textContent).toBe("<unsafe>actual result</unsafe>"); expect(el.querySelector("unsafe")).toBeNull();
    expect(el.textContent?.match(/Use compact expandable tool trees/g)).toHaveLength(1);
    expect(el.textContent).not.toMatch(/general|Task output|Tool details/);
    for (const childStatus of ["completed", "failed", "unknown"] as const) {
        act(() => root.render(<ToolPart part={part} onOpenSession={navigate} childStatus={childStatus} />));
        expect(el.querySelector(".tc-task-orbit")).toBeNull();
        expect(el.querySelector(childStatus === "failed" ? ".lucide-circle-x, .lucide-x-circle" : ".lucide-check")).not.toBeNull();
    }
    act(() => root.render(<ToolPart part={tool("task", { status: "pending", input: { description } })} onOpenSession={navigate} />));
    expect(el.querySelector(".tc-open-session")).toBeNull(); expect(el.querySelector(".tc-task-orbit")).not.toBeNull();
});
it("keeps failed calls compact and reveals the full error only on expansion", () => {
    const error = "bash command failed\n" + "source line\n".repeat(200) + "END_OF_OUTPUT";
    act(() => root.render(<ToolPart part={tool("bash",{status:"error",input:{command:"check"},error})} />));
    expect(el.querySelector(".tc-preview")?.textContent).toContain("bash command failed");
    expect(el.querySelector("pre")).toBeNull();
    expect(el.textContent).not.toContain("END_OF_OUTPUT");
    expect(el.querySelector(".tc-chevron")).toBeNull();
    act(() => el.querySelector<HTMLButtonElement>(".tc-preview")!.click());
    expect(el.querySelector("pre")?.textContent).toContain("END_OF_OUTPUT");
    expect(el.textContent).not.toContain("Copy visible");
});
it("ordinary read is one tree header without repeated path, labels or default output DOM", () => {
    const part = tool("read", { status: "completed", title: "Read src/skeleton.css", input: { filePath: "src/skeleton.css", limit: 40 }, output: "<script>unsafe()</script>\n  color: red;", time: { start: 0, end: 437 } });
    const html = renderToStaticMarkup(<ToolPart part={part} />);
    expect(html.match(/src\/skeleton.css/g)).toHaveLength(1);
    expect(html).not.toContain("Output preview"); expect(html).not.toContain("Tool details"); expect(html).not.toContain("<pre"); expect(html).toContain('aria-expanded="false"');
    act(() => root.render(<ToolPart part={part} />));
    expect(el.querySelector(".tc-status")?.textContent).toBe("");
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.querySelector(".tc-tree-body")).not.toBeNull(); expect(el.querySelector("script")).toBeNull();
    expect(el.querySelector("pre code")?.textContent).toBe("<script>unsafe()</script>\n  color: red;");
    expect(el.textContent?.match(/src\/skeleton.css/g)).toHaveLength(1);
    act(() => root.render(<ToolPart part={tool("read", { status: "error", input: { filePath: "x" }, error: "Denied" })} />));
    act(() => el.querySelector<HTMLButtonElement>(".tc-tool-toggle")!.click());
    expect(el.textContent).toContain("Denied"); expect(el.querySelector(".tc-tree-body")).toBeNull();
});
it("uses streamed task descriptions immediately and separates background child activity from completed call status", () => {
    const description = "Implement native background task and queue popovers";
    for (const raw of ['{"description":"Implement native', JSON.stringify({ description, subagent_type: "build" })]) {
        act(() => root.render(<ToolPart part={tool("functions.task", { status: "pending", input: {}, raw })} />));
        expect(el.querySelector(".tc-title")?.textContent).toContain("Task(Implement native");
    }
    act(() => root.render(<ToolPart part={tool("functions.task", { status: "running", input: { description }, time: { start: 1 } })} />));
    expect(el.querySelector(".tc-title")?.textContent).toBe(`Task(${description})`); expect(el.querySelector('[data-task-status="running"]')).not.toBeNull();
    act(() => root.render(<ToolPart part={tool("functions.task", { status: "completed", input: { description }, metadata: { sessionId: "ses_child", status: "running", background: true } })} />));
    expect(el.querySelector('[data-tool-status="completed"]')).not.toBeNull(); expect(el.querySelector('[data-task-status="running"]')).not.toBeNull();
});
