import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { coreCall } from "../src/native-core";
import { CommandController, type CommandOptions } from "../src/commands/CommandController";
import { KeyboardBindings, paletteActionForBinding, type KeyboardAction } from "../src/commands/KeyboardBindings";
const palettes: CommandController[] = [];
afterEach(() => palettes.splice(0).forEach((palette) => palette.dispose()));
const palette = (options: CommandOptions = {}) => { const result = new CommandController(options); palettes.push(result); return result; };
test("the native command catalog preserves original namespaces, labels, shortcuts, and actions", () => {
  const catalog = CommandController.catalog();
  expect(catalog.length).toBeGreaterThan(75);
  expect(catalog.find((c) => c.action === "SaveDocument")).toMatchObject({ title: "Write File", service: "code" });
  expect(catalog.find((c) => c.action === "SplitRight")?.title).toBe("Split Right");
  expect(catalog.some((c) => c.action === "LspCodeAction")).toBe(true);
  const commands = palette({ surface: "file" }); commands.query("Split Right");
  expect(commands.target()?.action).toBe("SplitRight"); expect(commands.frame.rows[0].displayTitle).toBe("neoism: Split Right");
});
test("surface and capability filters are the original palette policies", () => {
  const terminal = palette({ surface: "terminal" }); expect(terminal.frame.rows.some((r) => r.action === "SaveDocument")).toBe(false);
  const editor = palette({ surface: "file" }); expect(editor.frame.rows.some((r) => r.action === "SaveDocument")).toBe(true);
  expect(editor.frame.rows.some((r) => r.action === "RunNotebookCell")).toBe(false);
  const limited = palette({ surface: "file", capabilities: { settings_editor: false, lsp_actions: false, minimap: false } });
  expect(limited.frame.rows.some((r) => ["ConfigEditor", "OpenSettings", "LspHover", "ToggleMinimap"].includes(r.action || ""))).toBe(false);
  const notebook = palette({ surface: "notebook", capabilities: { notebook_kernel: false } });
  expect(notebook.frame.rows.some((r) => r.action === "RunNotebookCell")).toBe(true);
  expect(notebook.frame.rows.some((r) => r.action === "RunAllNotebookCells")).toBe(false);
});
test("command navigation and scrolling use the existing selection and cursor springs", () => {
  const commands = palette({ surface: "file" }); commands.move(20);
  expect(commands.frame.selected).toBe(20); expect(commands.frame.top).toBeGreaterThan(0); expect(commands.frame.animating).toBe(true);
  commands.query("Write File"); expect(commands.target()?.action).toBe("SaveDocument");
  commands.select(0); expect(commands.target()?.action).toBe("SaveDocument");
});
test("binding action aliases come from the desktop parser, including dynamic forms", () => {
  const aliases = coreCall<Record<string, string>>("commands.action-aliases"); expect(Object.keys(aliases).length).toBeGreaterThan(50);
  const parse = (action: string) => coreCall<KeyboardAction>("commands.parse-action", { action });
  expect(parse("CrEaTeWiNdOw")).toEqual({ kind: "action", action: "WindowCreateNew" });
  expect(parse("ToggleNeoismNotes")).toEqual({ kind: "action", action: "OpenNeoismNotes" });
  expect(parse("SearchFocusPrevious")).toEqual({ kind: "action", action: "SearchFocusPrevious" });
  expect(parse("SelectTab(3)")).toEqual({ kind: "action", action: "SelectTab", argument: 3 });
  expect(parse("Scroll(-12)")).toEqual({ kind: "action", action: "Scroll", argument: -12 });
  expect(parse("Run(Echo HELLO)")).toEqual({ kind: "action", action: "Run", program: "echo", args: ["hello"] });
  expect(parse("unrecognized")).toEqual({ kind: "action", action: "None" });
});
test("user key bindings keep modifier matching, raw escapes, mode guards, and default suppression", () => {
  const bindings = new KeyboardBindings([
    { key: "t", with: "control", action: "Quit" },
    { key: "t", with: "control", action: "CreateWorkspaceTerminalTab" },
    { key: "v", with: "command | shift", mode: "~vi", action: "Paste" },
    { key: "Home", with: "alt", action: "Quit", esc: "\u001bOH" },
    { key: "x", action: "None" },
  ]);
  expect(bindings.match({ key: "t", modifiers: { ctrl: true } })).toEqual([{ kind: "action", action: "WorkspaceTerminalTabCreateNew" }]);
  expect(bindings.match({ key: "T", modifiers: { ctrl: true, shift: true } })).toEqual([]);
  expect(bindings.match({ key: "v", modifiers: { cmd: true, shift: true } }, { vi: true })).toEqual([]);
  expect(bindings.match({ key: "v", modifiers: { cmd: true, shift: true } }, { vi: false })).toEqual([{ kind: "action", action: "Paste" }]);
  expect(bindings.match({ key: "home", modifiers: { alt: true } })).toEqual([{ kind: "escape", text: "\u001bOH" }]);
  expect(bindings.match({ key: "x" })).toEqual([{ kind: "action", action: "None" }]);
  expect(paletteActionForBinding("WorkspaceTerminalTabCreateNew")).toBe("TabCreate");
});
test("mode-specific overrides retain the desktop conflict rule and multiple intentional matches", () => {
  const bindings = new KeyboardBindings([
    { key: "k", mode: "vi", action: "Copy" }, { key: "k", mode: "~vi", action: "Paste" },
    { key: "g", mode: "vi", action: "Copy" }, { key: "g", mode: "appcursor", action: "Paste" },
  ]);
  expect(bindings.match({ key: "k" }, { vi: true })).toEqual([{ kind: "action", action: "Copy" }]);
  expect(bindings.match({ key: "k" }, { vi: false })).toEqual([{ kind: "action", action: "Paste" }]);
  expect(bindings.match({ key: "g" }, { vi: true, appcursor: true })).toEqual([{ kind: "action", action: "Copy" }, { kind: "action", action: "Paste" }]);
});
test("JSONC binding configuration is read without rewriting it", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-bindings-")), path = join(directory, "config.json");
  const original = '{ // user config\n "keybinds":{"keys":[{"key":"e","with":"alt","action":"OpenCommandPalette"},]},}';
  try {
    writeFileSync(path, original); const bindings = KeyboardBindings.fromConfig(path);
    expect(readFileSync(path, "utf8")).toBe(original);
    expect(bindings.warnings).toEqual([]); expect(bindings.match({ key: "e", modifiers: { alt: true } })).toEqual([{ kind: "action", action: "OpenCommandPalette" }]);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
