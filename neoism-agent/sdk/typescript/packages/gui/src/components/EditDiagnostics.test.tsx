// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it } from "vitest";
import { EditDiagnostics } from "./EditDiagnostics";
import { editDiagnosticPathMatches as matches, readEditDiagnostics } from "./editDiagnosticsData";
import type { CardPart } from "./toolCardData";

const issue = (extra: Record<string, unknown> = {}) => ({ message: "Cannot find name 'missing'.", severity: "error", range: { start: { line: 30, character: 166 }, end: { line: 30, character: 173 } }, source: "ts", code: "2304", related_information: [], tags: [], ...extra });
const entry = (path = "src/app.ts", diagnostics: unknown[] = [issue()], extra = {}) => ({ path, diagnosticsKind: "cached", freshness: "unknown", source: "touched", errorCount: 1, warningCount: 0, diagnostics, ...extra });
const part = (metadata: unknown = { diagnostics: [entry()] }, input: unknown = { filePath: "/repo/src/app.ts" }, output = ""): CardPart => ({ type: "tool", id: "p", tool: "edit", state: { status: "completed", input, metadata, output } } as CardPart);
let root: Root, el: HTMLDivElement;
beforeEach(() => { (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true; el = document.createElement("div"); document.body.append(el); root = createRoot(el); });
afterEach(() => { act(() => root.unmount()); el.remove(); });
const render = (p: CardPart, paths?: string[]) => act(() => root.render(<EditDiagnostics part={p} paths={paths} />));

it("renders native ERROR [31:167] with cache freshness in a tooltip only", () => {
    render(part({ diagnostics: [entry(), entry("src/other.ts", [issue({ message: "Unrelated project error" })], { source: "project" })] }));
    expect(el.textContent).toContain("ERROR [31:167] Cannot find name 'missing'.");
    expect(el.textContent).not.toMatch(/Unrelated|freshness|errorCount|Browse|src\/app.ts/);
    expect(el.querySelector(".edit-diagnostics-file")?.getAttribute("title")).toContain("may predate this edit");
    expect(el.querySelector(".edit-diagnostics-file")?.getAttribute("title")).toContain("not confirmation");
    expect(el.textContent).not.toContain("Cached diagnostics");
    expect(el.querySelector('[data-severity="error"]')).not.toBeNull();
});
it("uses component-aware relative/absolute suffixes and Windows paths without conflating roots", () => {
    for (const [a, b] of [["src/app.ts", "/repo/src/app.ts"], ["src/app.ts", "C:\\Repo\\SRC\\app.ts"], ["\\\\Host\\Share\\src\\app.ts", "src/app.ts"], ["./src/app.ts", "src/app.ts"]]) expect(matches(a, b)).toBe(true);
    for (const [a, b] of [["src/app.ts", "/repo/not-src/app.ts"], ["app.ts", "myapp.ts"], ["/one/src/app.ts", "/two/src/app.ts"], ["src/app.ts", "other/src/app.ts"], ["/repo/A.ts", "/repo/a.ts"], ["", "src/app.ts"]]) expect(matches(a, b)).toBe(false);
});
it("derives input and patch paths, accepts move/source paths, and adds file headers only for multiple files", () => {
    const p = part({ diagnostics: [entry("old.ts"), entry("new.ts"), entry("third.ts")] }, { path: "old.ts" });
    render(p, ["new.ts", "third.ts"]);
    expect([...el.querySelectorAll(".edit-diagnostics-path")].map(e => e.textContent)).toEqual(["old.ts", "new.ts", "third.ts"]);
    const patch = { ...part({ diagnostics: [entry("new.ts")] }, { patchText: "*** Begin Patch\n*** Update File: old.ts\n*** Move to: new.ts\n@@\n-old\n+new\n*** End Patch" }), tool: "apply_patch" } as CardPart;
    expect(readEditDiagnostics(patch)).toHaveLength(1);
    expect(readEditDiagnostics(part({ diagnostics: [entry("old.ts"), entry("new.ts")] }, { sourcePath: "old.ts", moveTo: "new.ts" }))).toHaveLength(2);
});
it("keeps primary errors/warnings, sorts errors first, deduplicates full issue identity, and ignores related/hint spam", () => {
    const warning = issue({ severity: "warning", message: "unused variable", related_information: [{ message: "unused variable" }] });
    render(part({ diagnostics: [entry("src/app.ts", [warning, warning, issue(), issue(), issue({ severity: "hint", message: "inactive code" }), issue({ severity: "information", message: "FYI" })]), entry("/repo/src/app.ts", [warning])] }));
    const rows = [...el.querySelectorAll(".edit-diagnostics-row")];
    expect(rows).toHaveLength(2); expect(rows[0].textContent).toContain("ERROR"); expect(rows[1].textContent).toContain("WARN [31:167] unused variable");
    expect(rows[1].getAttribute("data-severity")).toBe("warning"); expect(el.textContent).not.toMatch(/inactive|FYI/);
    expect(readEditDiagnostics(part({ diagnostics: [entry("src/app.ts", [issue(), issue({ code: "different" })])] }))[0].diagnostics).toHaveLength(2);
});
it("validates malformed shapes, keeps zero-based line zero correct, and never displays NaN", () => {
    render(part({ diagnostics: [null, {}, entry("src/app.ts", [null, {}, issue({ severity: "bogus" }), issue({ severity: 1 }), issue({ message: {} }), issue({ range: { start: { line: 0, character: 0 } } }), issue({ message: "malformed", range: { start: { line: NaN, character: -4 } } })])] }));
    expect(el.querySelectorAll(".edit-diagnostics-row")).toHaveLength(2);
    expect(el.textContent).toContain("ERROR [1:1]"); expect(el.textContent).not.toContain("NaN");
    for (const diagnostics of [null, {}, "bad", []]) expect(readEditDiagnostics(part({ diagnostics }))).toEqual([]);
});
it("React-escapes paths and full multiline messages, strips ANSI/OSC, and never turns URLs into links", () => {
    const path = 'src/<img onerror="bad">&.ts';
    const message = '\x1b[31m<script>bad()</script>\x1b[0m\n' + "long message ".repeat(1000) + '\x1b]8;;https://example.com\x07https://example.com\x1b]8;;\x07';
    render(part({ diagnostics: [entry(path, [issue({ message })]), entry("second.ts")] }, { path }), ["second.ts"]);
    expect(el.textContent).toContain(path); expect(el.textContent).toContain("<script>bad()</script>\n");
    expect(el.textContent).toContain("long message ".repeat(1000)); expect(el.textContent).not.toContain("\x1b");
    expect(el.querySelector("script, img, a")).toBeNull();
});
const report = '<diagnostics file="src/app.ts">\nERROR [31:167] Cannot find name.\n</diagnostics>';
it("supports strict legacy native reports only for actual file tools, decoding escaped attributes", () => {
    render(part({}, undefined, "Edited successfully.\nCached LSP errors (may predate this edit; verify before fixing):\n" + report));
    expect(el.textContent).toContain("ERROR [31:167] Cannot find name."); expect(el.textContent).not.toContain("Cached diagnostics");
    expect(readEditDiagnostics(part({}, undefined, report))).toHaveLength(1);
    const multiline = report.replace("Cannot find name.", "Cannot find name.\n  more detail\nHINT [1:1] inactive code");
    expect(readEditDiagnostics(part({}, undefined, multiline))[0].diagnostics.map(row => row.message)).toEqual(["Cannot find name.\n  more detail"]);
    expect(readEditDiagnostics(part({}, { path: 'a&b".ts' }, '<diagnostics file="a&amp;b&quot;.ts">\nWARN [1:1] Warning\n</diagnostics>'))).toHaveLength(1);
    for (const output of ["// " + report, "```ts\n" + report + "\n```", "/*\n" + report + "\n*/", "Some prose\n" + report, report + "\nmore prose", report.replace("[31:167]", "[NaN:0]")]) expect(readEditDiagnostics(part({}, undefined, output))).toEqual([]);
    expect(readEditDiagnostics({ ...part({}, undefined, report), tool: "bash" } as CardPart)).toEqual([]);
    expect(readEditDiagnostics(part({}, { path: "other.ts" }, report))).toEqual([]);
});
it("honors empty metadata over stale legacy output and does not present tool status errors", () => {
    for (const diagnostics of [[], [entry("src/app.ts", [])], null]) { render(part({ diagnostics }, undefined, report)); expect(el.innerHTML).toBe(""); }
    render({ ...part(), state: { status: "error", error: "Permission denied", input: {}, metadata: {} } } as CardPart);
    expect(el.innerHTML).toBe("");
    render(part({ diagnostics: [entry("src/app.ts", [issue()], { diagnosticsKind: "live", freshness: "current" })] }));
    expect(el.querySelector(".edit-diagnostics-cache")).toBeNull();
});
it("caps initial rows across files to 100, expands compactly, and resets for another part", () => {
    const many = part({ diagnostics: [entry("src/app.ts", Array.from({ length: 105 }, (_, i) => issue({ message: `Error ${i}` })))] });
    render(many); expect(el.querySelectorAll(".edit-diagnostics-row")).toHaveLength(100);
    expect(el.querySelector("button")?.textContent).toContain("5 more");
    act(() => el.querySelector("button")!.click()); expect(el.querySelectorAll(".edit-diagnostics-row")).toHaveLength(105);
    render({ ...many, id: "next" }); expect(el.querySelectorAll(".edit-diagnostics-row")).toHaveLength(100);
});
