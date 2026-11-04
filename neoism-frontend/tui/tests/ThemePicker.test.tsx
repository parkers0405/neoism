import { afterEach, expect, test } from "bun:test";
import { createSignal } from "solid-js";
import { testRender } from "@opentui/solid";
import { RGBA, type BoxRenderable, type InputRenderable, type TextRenderable } from "@opentui/core";
import { ThemePicker, type ThemePickerProps } from "../src/ThemePicker";
import { palettes } from "../../gpuix/src/design/generated";
import { extractThemePreview, themePreview } from "../src/theme-preview";

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0)) await cleanup(); });
async function setup(overrides: Partial<ThemePickerProps> = {}) {
  let applied = "pastel_dark", submissions = 0;
  const names = Object.keys(palettes);
  const [props, update] = createSignal<ThemePickerProps>({ themes: names, selected: 0, query: "", width: 100, height: 26, theme: palettes.pastel_dark,
    onQuery: query => update(p => ({ ...p, query, themes: names.filter(name => name.includes(query)), selected: 0 })),
    onSelect: selected => update(p => ({ ...p, selected })),
    onSubmit: () => { applied = props().themes[Math.max(0, Math.min(props().themes.length - 1, props().selected))]!; submissions++; }, ...overrides });
  const screen = await testRender(() => <ThemePicker {...props()} />, { width: props().width, height: props().height });
  cleanups.push(() => screen.dispose());
  await screen.flush();
  return { ...screen, props, update, applied: () => applied, submissions: () => submissions,
    node: (id: string) => screen.renderer.root.findDescendantById(`theme-picker${id ? `-${id}` : ""}`)! };
}

test("left list and real right palette preview; selection never applies until submit", async () => {
  const s = await setup();
  expect(s.node("preview").x).toBe(s.node("list").x + s.node("list").width);
  expect(s.node("preview").y).toBe(s.node("list").y);
  expect(s.node("list").width / (s.node("").width - 2)).toBeCloseTo(.3, 1);
  s.update(p => ({ ...p, selected: 2 })); await s.flush();
  const palette = palettes.tokyo_night;
  expect((s.node("preview") as BoxRenderable).backgroundColor).toEqual(RGBA.fromHex(palette.bg));
  expect((s.node("name") as TextRenderable).fg).toEqual(RGBA.fromHex(palette.fg));
  expect(s.captureCharFrame()).toContain("Tokyo Night");
  for (const [row, runs] of themePreview.entries()) {
    const line = s.node(`code-${row}`) as TextRenderable;
    expect(line.x).toBeGreaterThan(s.node("list").x + s.node("list").width);
    expect(line.y).toBeLessThan(s.node("preview").y + s.node("preview").height);
    // TextNode can merge adjacent equal-colored runs; compare each rendered
    // character instead of assuming token boundaries survive that optimization.
    const actual = line.textNode.toChunks().flatMap(chunk => [...chunk.text].map(char => [char, chunk.fg]));
    const expected = runs.flatMap(run => [...run.text].map(char => [char, RGBA.fromHex(palette[run.token])]));
    expect(actual).toEqual(expected);
    expect(s.captureCharFrame()).toContain(runs.map(run => run.text).join(""));
  }
  expect((s.node("divider") as TextRenderable).fg).toEqual(RGBA.fromHex(palette.border));
  expect(s.captureCharFrame()).not.toMatch(/main\.ts|TypeScript|NORMAL|1:1|README/);
  expect(s.applied()).toBe("pastel_dark"); expect(s.submissions()).toBe(0);
  (s.node("input") as InputRenderable).submit();
  expect(s.applied()).toBe("tokyo_night"); expect(s.submissions()).toBe(1);
});

test("preview is the native eight-line Rust sample, including exact token boundaries", () => {
  expect(themePreview.map(row => row.map(run => run.text).join(""))).toEqual([
    "pub struct Theme {", "    name: String,", "}", "// Live syntax preview",
    'let theme = Theme::load("neoism");', "if theme.contrast > 4.5 {", "    render(&theme);", "}",
  ]);
  expect(themePreview[0]).toEqual([
    { text: "pub ", token: "syn_keyword" }, { text: "struct ", token: "syn_statement" },
    { text: "Theme ", token: "syn_type" }, { text: "{", token: "fg" },
  ]);
  expect(() => extractThemePreview("changed source")).toThrow("painter not found");
});

test("query filtering is controlled by parent, empty matches cannot submit", async () => {
  const s = await setup();
  (s.node("input") as InputRenderable).insertText("tokyo"); await s.flush();
  expect(s.props().query).toBe("tokyo");
  expect(s.props().themes).toEqual(Object.keys(palettes).filter(name => name.includes("tokyo")));
  expect(s.captureCharFrame()).toContain("tokyo_night");
  expect(s.applied()).toBe("pastel_dark");
  s.props().onQuery("no-such-theme"); await s.flush();
  expect(s.captureCharFrame()).toContain("No matches");
  expect(s.renderer.root.findDescendantById("theme-picker-code")).toBeUndefined();
  (s.node("input") as InputRenderable).submit(); expect(s.submissions()).toBe(0);
});

test("selected scroll window preserves absolute indices; hover previews and row click submits", async () => {
  const s = await setup({ selected: 25 });
  expect(s.node("row-25")).toBeDefined();
  expect(s.renderer.root.findDescendantById("theme-picker-row-0")).toBeUndefined();
  const row = s.node("row-26");
  const x = row.x + 2, y = row.y;
  await s.mockMouse.moveTo(x, y); await s.flush();
  expect(s.props().selected).toBe(26); expect(s.submissions()).toBe(0);
  await s.mockMouse.click(x, y); await s.flush();
  expect(s.applied()).toBe(s.props().themes[26]!); expect(s.submissions()).toBe(1);
  s.update(p => ({ ...p, selected: 999 })); await s.flush();
  expect(s.node(`row-${s.props().themes.length - 1}`)).toBeDefined();
  s.update(p => ({ ...p, selected: -4 })); await s.flush();
  expect(s.captureCharFrame()).toContain("› pastel_dark");
});

test("wide dialog capped at 110; narrow layout stacks with a visible list, all columns fit", async () => {
  for (const width of [160, 52, 51, 30, 12]) {
    const s = await setup({ width, height: 22 });
    expect(s.node("").width).toBe(Math.min(width - 4, 110));
    const list = s.node("list"), preview = s.node("preview"), dialog = s.node("");
    expect(list.height).toBeGreaterThanOrEqual(3);
    if (width >= 52) expect(preview.x).toBe(list.x + list.width);
    else expect(preview.y).toBe(list.y + list.height);
    for (const child of [list, preview]) {
      expect(child.x).toBeGreaterThanOrEqual(dialog.x);
      expect(child.x + child.width).toBeLessThanOrEqual(dialog.x + dialog.width);
      expect(child.y + child.height).toBeLessThanOrEqual(22);
    }
    expect(s.node("row-0")).toBeDefined();
  }
});