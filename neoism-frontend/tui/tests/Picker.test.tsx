import { afterEach, expect, test } from "bun:test";
import { createSignal } from "solid-js";
import { testRender } from "@opentui/solid";
import type { BoxRenderable, InputRenderable, TextRenderable } from "@opentui/core";
import { RGBA } from "@opentui/core";
import { Picker, type PickerProps } from "../src/Picker";
import { palettes } from "../../gpuix/src/design/generated";

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0)) await cleanup(); });
async function setup(overrides: Partial<PickerProps> = {}) {
  const [props, update] = createSignal<PickerProps>({ title: "Commands", query: "", onQuery: value => update(p => ({ ...p, query: value })),
    items: [{ id: "save", title: "Save file", key: "Ctrl+S", glyph: "\uf15b", color: "#9fe8c3" }, { id: "search", title: "Search files", key: "Alt+S" }],
    selected: 0, onSelect: selected => update(p => ({ ...p, selected })), onSubmit: () => {}, width: 48, height: 20, ...overrides });
  const screen = await testRender(() => <Picker {...props()} />, { width: props().width, height: props().height });
  cleanups.push(() => screen.dispose());
  await screen.flush();
  return { ...screen, update, props, node: (id: string) => screen.renderer.root.findDescendantById(id)! };
}

test("rounded separate black prompt/results, centered at twenty percent with exact icon color", async () => {
  const s = await setup();
  expect([s.node("picker").x, s.node("picker").y, s.node("picker").width]).toEqual([2, 4, 44]);
  const prompt = s.node("picker-prompt") as BoxRenderable, results = s.node("picker-results") as BoxRenderable;
  expect([prompt.height, results.y, results.height]).toEqual([3, 7, 4]);
  expect(prompt.borderStyle).toBe("rounded");
  expect(results.borderStyle).toBe("rounded");
  expect(prompt.backgroundColor.toString()).toBe(results.backgroundColor.toString());
  expect(prompt.backgroundColor).toEqual(RGBA.fromHex("#000000"));
  expect(prompt.borderColor).toEqual(RGBA.fromHex(palettes.pastel_dark.blue));
  expect((s.node("picker-glyph-0") as TextRenderable).fg).toEqual(RGBA.fromHex("#9fe8c3"));
  const frame = s.captureCharFrame().split("\n").slice(4, 11).map(line => line.trimEnd()).join("\n");
  expect(frame).toBe([
    "  ╭─ Commands ───────────────────────────────╮",
    "  │  Type a command…                        │",
    "  ╰──────────────────────────────────────────╯",
    "  ╭──────────────────────────────────────────╮",
    "  │  Save file                     Ctrl+S  │",
    "  │  Search files                     Alt+S  │",
    "  ╰──────────────────────────────────────1/2─╯",
  ].join("\n"));
});

test("selected scrollwindow is capped at twelve and clicking keeps absolute indexes", async () => {
  const s = await setup({ height: 40, selected: 18, items: Array.from({ length: 30 }, (_, i) => ({ id: String(i), title: `File ${i}` })) });
  expect(s.node("picker-results").height).toBe(14);
  expect(s.captureCharFrame()).toContain("\uf054 File 18");
  expect(s.captureCharFrame()).not.toContain("File 0 ");
  const row = s.node("picker-row-19");
  await s.mockMouse.click(row.x + 2, row.y); await s.flush();
  expect(s.props().selected).toBe(19);
  s.update(p => ({ ...p, selected: 29 })); await s.flush();
  expect(s.captureCharFrame()).toContain("\uf054 File 29");
});

test("narrow search preserves path tail, clips details and reacts to query and palette", async () => {
  const s = await setup({ width: 28, search: true, items: [{ id: "a", title: "very/long/path/文件.ts", key: "a long detail", glyph: "\uf15b", color: "#ef8891" }] });
  expect(s.node("picker").width).toBe(24);
  expect(s.captureCharFrame()).toContain("…");
  expect(s.captureCharFrame()).not.toContain("very/long/path");
  const input = s.node("picker-input") as InputRenderable;
  expect(input.focused).toBe(true);
  input.insertText("src"); await s.flush();
  expect(s.props().query).toBe("src");
  s.update(p => ({ ...p, query: "lib", theme: { ...palettes.pastel_dark, bg: "#112233", blue: "#aabbcc" } })); await s.flush();
  expect(input.value).toBe("lib");
  expect((s.node("picker-prompt") as BoxRenderable).backgroundColor).toEqual(RGBA.fromHex("#112233"));
  expect((s.node("picker-glyph-0") as TextRenderable).fg).toEqual(RGBA.fromHex("#ef8891"));
});

test("busy/error are border footers, never invented results; input-only stays compact", async () => {
  const s = await setup({ items: [], busy: true });
  expect(s.captureCharFrame()).toContain("Loading…");
  expect(s.node("picker-results").getChildren()).toHaveLength(0);
  s.update(p => ({ ...p, busy: false, error: "Search failed" })); await s.flush();
  expect(s.captureCharFrame()).toContain("Search failed");
  s.update(p => ({ ...p, inputOnly: true })); await s.flush();
  expect(s.renderer.root.findDescendantById("picker-results")).toBeUndefined();
  expect(s.node("picker-prompt").height).toBe(3);
});

test("prompt submission delegates without owning navigation; large and short terminals fit", async () => {
  let submissions = 0;
  const s = await setup({ width: 160, height: 8, onSubmit: () => { submissions++; } });
  expect(s.node("picker").width).toBe(84);
  expect(s.node("picker").x).toBe(38);
  const results = s.node("picker-results");
  expect(results.y + results.height).toBeLessThanOrEqual(8);
  (s.node("picker-input") as InputRenderable).submit();
  expect(submissions).toBe(1);
  expect(s.props().selected).toBe(0);
  s.update(p => ({ ...p, width: 12, height: 4 })); await s.flush();
  expect(s.node("picker").width).toBe(8);
  expect(s.renderer.root.findDescendantById("picker-results")).toBeUndefined();
});