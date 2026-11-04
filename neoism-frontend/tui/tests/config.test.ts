import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadTuiConfig, persistTheme } from "../src/config";
import { palettes } from "../../gpuix/src/design/generated";
import { TuiModel } from "../src/model";
import { until } from "./support/model-fixture";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function configFile(source: string) {
  const root = mkdtempSync(join(tmpdir(), "neoism-tui-config-")); roots.push(root);
  const path = join(root, "config.json"); writeFileSync(path, source);
  return { path, env: { NEOISM_CONFIG_HOME: root } };
}

test("shared JSONC supplies grouped appearance, Vim and keyboard configuration", () => {
  const { path, env } = configFile(`{
    // Desktop and TUI share these settings.
    "appearance": { "theme": "pastel_dark", },
    "editor": { "vim-mode": false },
    "keybinds": { "keys": [{ "key": "p", "with": "Alt", "action": "None" }] },
  }`);
  const config = loadTuiConfig(["--offline"], env);
  expect(config.configPath).toBe(path);
  expect(config.theme).toEqual(palettes.pastel_dark);
  expect(config.vim).toBe(false);
  expect(config.keybinds).toEqual([{ key: "p", with: "Alt", action: "None" }]);
  expect(config.warnings).toEqual([]);
});

test("theme persistence preserves comments and unrelated desktop settings", () => {
  const { path, env } = configFile('{\n // retain me\n "appearance": { "theme": "pastel_dark", "font-size": 17 },\n "editor": { "vim-mode": false }\n}\n');
  const nextTheme = Object.keys(palettes).find(name => name !== "pastel_dark")!;
  persistTheme({ configPath: path }, nextTheme);
  expect(readFileSync(path, "utf8")).toContain("// retain me");
  expect(readFileSync(path, "utf8")).toContain('"font-size": 17');
  const config = loadTuiConfig(["--offline"], env);
  expect(config.themeName).toBe(nextTheme);
  expect(config.vim).toBe(false);
  expect(loadTuiConfig(["--offline", "--theme", "pastel_dark"], env).themeName).toBe("pastel_dark");
});

test("invalid JSONC is reported and never overwritten by a theme change", () => {
  const source = '{ "appearance": broken }';
  const { path, env } = configFile(source);
  expect(loadTuiConfig(["--offline"], env).warnings).toContain("Invalid JSONC config; using appearance defaults.");
  expect(() => persistTheme({ configPath: path }, "pastel_dark")).toThrow("invalid JSONC");
  expect(readFileSync(path, "utf8")).toBe(source);
  expect(() => persistTheme({ configPath: path }, "not-a-theme")).toThrow("Unknown theme");
});

test("shared config hot-reloads theme, Vim and bindings; TUI toggles persist in the same grouped file", async () => {
  const { path, env } = configFile('{ // shared comment\n "appearance": {"theme":"pastel_dark"}, "editor":{"vim-mode":true}, "agent":{"keep":"untouched"} }');
  const model = new TuiModel(loadTuiConfig(["--offline"], env));
  model.start();
  try {
    writeFileSync(path, '{ // shared comment\n "appearance":{"theme":"tokyo_night"}, "editor":{"vim-mode":false}, "keybinds":{"keys":[{"key":"t","with":"Alt","action":"OpenThemePicker"}]}, "agent":{"keep":"untouched"} }');
    await until(() => model.config.themeName === "tokyo_night" && !model.config.vim, "Shared config reload", 3000);
    expect(model.config.keybinds).toEqual([{ key: "t", with: "Alt", action: "OpenThemePicker" }]);
    model.setVim(true);
    expect(loadTuiConfig(["--offline"], env).vim).toBe(true);
    expect(readFileSync(path, "utf8")).toContain("shared comment");
    expect(readFileSync(path, "utf8")).toContain('"keep":"untouched"');
    await model.setTheme("pastel_dark");
    expect(palettes.pastel_dark.bg).toBe("#000000");
    expect(loadTuiConfig(["--offline"], env).themeName).toBe("pastel_dark");
  } finally { model.stop(); }
});