import { expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { isolatedDaemon } from "./support/isolated-daemon";
import { until } from "./support/model-fixture";

// No protocol peer: the model's own TuiLsp uses the shared service client,
// an ephemeral workspace daemon and the installed clangd over real stdio.
test("real clangd: TUI hover, completion, definition, diagnostics and unsaved didChange", async () => {
  expect(Bun.which("clangd"), "Install/provide clangd on PATH; this test must not silently skip").not.toBeNull();
  const fixture = await isolatedDaemon();
  const original = "struct Pair { int before; };\nstruct Pair value;\nint read_value(void) { return value.before; }\nint broken = missing_name;\n";
  const updated = "struct Pair { double after; };\nstruct Pair value;\nint read_value(void) { return value.after; }\nint broken = missing_name;\n";
  try {
    const path = join(fixture.root, "main.c");
    writeFileSync(path, original);
    const model = fixture.make();
    await until(() => !!model.state.directories[""], "TUI discovers isolated workspace", 8000);
    const buffer = await model.openFile("main.c");
    expect(buffer).toBeDefined();
    if (!buffer) throw new Error(model.state.error || "TUI failed to open main.c");
    await until(() => model.documents.isSeeded(buffer.editor), "Native document seeds", 5000);
    expect(buffer.editor.text).toBe(original);

    // Wait for a genuine server diagnostic, not merely an empty initial batch.
    await until(() => !!buffer.diagnostics?.some(item => item.message.includes("missing_name")), "clangd publishes missing_name diagnostic", 12000);
    expect(buffer.diagnostics).toContainEqual(expect.objectContaining({ severity: "Error", line: 3 }));
    const member = { line: 2, col: "int read_value(void) { return value.".length };
    const hover = await model.lsp.hover(buffer.id, member);
    expect(hover?.text).toMatch(/\bint\b/);
    expect(hover?.text).toContain("before");
    const completion = await model.lsp.completion(buffer.id, member, ".");
    expect(completion.items.some(item => item.label.includes("before"))).toBe(true);
    expect(await model.lsp.definition(buffer.id, member)).toContainEqual({ path, line: 0, col: "struct Pair { int ".length });

    // Native editor -> TuiModel.changed -> TuiLsp.update -> daemon didChange.
    // Keep disk unchanged so a server re-reading the file cannot pass this check.
    model.callEditor("apply-edits", { edits: [{ start_line: 0, start_col: 0, end_line: 4, end_col: 0, text: updated }] });
    expect(buffer.editor.text).toBe(updated);
    expect(readFileSync(path, "utf8")).toBe(original);
    const changedHover = await model.lsp.hover(buffer.id, member);
    expect(changedHover?.text).toMatch(/\bdouble\b/);
    expect(changedHover?.text).toContain("after");
    expect(changedHover?.text).not.toContain("before");
    const changedCompletion = await model.lsp.completion(buffer.id, member, ".");
    expect(changedCompletion.items.some(item => item.label.includes("after"))).toBe(true);
    expect(changedCompletion.items.some(item => item.label.includes("before"))).toBe(false);
    expect(model.state.error).toBeUndefined();
  } catch (error) {
    throw new Error(`${error}\nArtifact: ${fixture.executable}\n${fixture.output().slice(-6000)}`, { cause: error });
  } finally { await fixture.dispose(); }
}, 45000);