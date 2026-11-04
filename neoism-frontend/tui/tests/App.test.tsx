import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testRender } from "@opentui/solid";
import { App } from "../src/App";
import { loadTuiConfig } from "../src/config";
import { TuiModel } from "../src/model";
import { modelFixture, until } from "./support/model-fixture";
import { palettes } from "../../gpuix/src/design/generated";
import { RGBA, type BoxRenderable } from "@opentui/core";
import { icons, fileIcon } from "../src/icons";

const disposals: Array<() => void | Promise<void>> = [];
afterEach(async () => { for (const dispose of disposals.splice(0).reverse()) await dispose(); });

async function setup(width = 120, height = 40, options: { offline?: boolean; config?: unknown; manyFiles?: boolean; rootEntries?: Array<{ name: string; is_dir: boolean }> } = {}) {
  const configHome = mkdtempSync(join(tmpdir(), "neoism-tui-ui-"));
  disposals.push(() => rmSync(configHome, { recursive: true, force: true }));
  writeFileSync(join(configHome, "config.json"), JSON.stringify(options.config ?? { appearance: { theme: "pastel_dark" } }));
  const fixture = options.offline ? undefined : modelFixture({ configHome, rootEntries: options.rootEntries || (options.manyFiles ? Array.from({ length: 80 }, (_, index) => ({ name: `file-${String(index).padStart(3, "0")}.txt`, is_dir: false })) : undefined) });
  if (fixture) disposals.push(() => fixture.dispose());
  const model = fixture?.model ?? new TuiModel(loadTuiConfig(["--offline"], { NEOISM_CONFIG_HOME: configHome }));
  const screen = await testRender(() => <App model={model} />, { width, height, enableMouseMovement: true });
  disposals.push(() => screen.dispose());
  if (fixture) await until(() => !!model.state.activeWorkspace, "Workspace loaded");
  await screen.flush();
  const settle = async () => { await Bun.sleep(35); await screen.flush(); };
  const escape = async () => { screen.mockInput.pressEscape(); await Bun.sleep(60); await screen.flush(); };
  return { ...screen, model, fixture: fixture!, configHome, settle, escape };
}

test("wide and narrow splash preserve chrome, navigation and resize", async () => {
  const screen = await setup(120, 40, { offline: true });
  let frame = screen.captureCharFrame();
  expect(screen.renderer.root.findDescendantById("topbar")).toBeUndefined();
  expect(frame).not.toContain("NEOISM");
  expect(frame).not.toContain("Offline preview");
  expect(frame).toContain("Open file tree");
  expect(frame).toContain("Offline");
  expect(frame).not.toContain(icons.workspace);
  const image = screen.renderer.root.findDescendantById("splash-wordmark") as BoxRenderable;
  expect(image).toBeDefined();
  await screen.flush();
  if (process.env.NEOISM_TUI_TEST_FRAME) console.log(screen.captureCharFrame());
  const ink = screen.captureSpans().lines.slice(image.y, image.y + image.height).flatMap(line => line.spans)
    .filter(span => /[█▀▄]/.test(span.text) && span.fg.equals(RGBA.fromHex(palettes.pastel_dark.fg)) && !span.fg.equals(span.bg));
  expect(ink.length).toBeGreaterThan(0);
  expect(frame.split("\n")[0]).not.toContain("[Alt");
  expect(frame).not.toContain("Neoism ·");
  expect(frame).toContain(icons.search);
  expect(screen.captureSpans().lines[0]!.spans.every(span => span.bg.equals(RGBA.fromHex(palettes.pastel_dark.bg)))).toBe(true);
  expect(screen.captureSpans().lines.at(-1)!.spans.every(span => span.bg.equals(RGBA.fromHex(palettes.pastel_dark.bg)))).toBe(true);
  screen.resize(60, 20); await screen.flush();
  frame = screen.captureCharFrame();
  expect(frame).toContain("[Alt + P]");
  expect(frame).not.toContain("Offline");
  screen.mockInput.pressTab(); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Offline");
  screen.mockInput.pressTab(); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Open file tree");
  screen.resize(120, 40); await screen.flush();
  expect(screen.captureCharFrame()).toContain("Offline");
});

test("workspace and editable buffer tabs, breadcrumbs, home and keyboard cycling", async () => {
  const screen = await setup();
  await screen.model.openFile("src/code.rs");
  await screen.model.openFile("z.txt");
  await screen.settle();
  let frame = screen.captureCharFrame();
  expect(frame).toContain("project");
  expect(frame).not.toContain("connected");
  expect(frame.trimEnd().split("\n").at(-2)).toBe("─".repeat(120));
  expect(frame).toContain("code.rs");
  expect(frame).toContain("z.txt");
  expect(frame).toContain("content of z.txt");
  expect(frame).not.toContain("READ ONLY");
  // Legacy Shift+Tab cannot carry Control; send the terminal's CSI-u encoding.
  await screen.mockInput.pressKeys(["\x1b[9;6u"]); await screen.settle();
  expect(screen.model.activeBuffer?.path).toBe("src/code.rs");
  expect(screen.captureCharFrame()).toContain(`${icons.folder} src ${icons.chevronRight} ${fileIcon("code.rs").glyph} code.rs`);
  screen.mockInput.pressKey("0", { meta: true }); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Open file tree");
  screen.model.selectBuffer("src/code.rs"); await screen.settle();
  expect(screen.captureCharFrame()).toContain("content of src/code.rs");
  screen.mockInput.pressKey("w", { ctrl: true }); await screen.settle();
  expect(screen.model.state.buffers.map(buffer => buffer.path)).toEqual(["z.txt"]);
  await screen.mockInput.pressKeys(["\x1b[93;3u"]); await screen.settle();
  expect(screen.model.state.activeWorkspace?.id).toBe("second");
});

test("mockInput drives native Vim mode, cursor, dirty text and Ctrl+S CRDT save", async () => {
  const screen = await setup();
  await screen.model.openFile("z.txt"); await screen.settle();
  expect(screen.captureCharFrame()).toContain("NORMAL");
  screen.mockInput.pressKey("i"); await screen.settle();
  expect(screen.captureCharFrame()).toContain("INSERT");
  await screen.mockInput.typeText("hello "); await screen.settle();
  expect(screen.model.activeBuffer?.editor.text).toBe("hello content of z.txt");
  expect(screen.model.activeBuffer?.editor.frame.cursor.col).toBe(6);
  expect(screen.model.activeBuffer?.editor.frame.dirty).toBe(true);
  expect(screen.captureCharFrame()).toContain("1:7");
  await screen.escape();
  expect(screen.captureCharFrame()).toContain("NORMAL");
  screen.mockInput.pressKey("s", { ctrl: true });
  await until(() => screen.fixture.saved.size === 1 && !screen.model.activeBuffer?.editor.frame.dirty, "Native buffer saved");
  expect([...screen.fixture.saved.values()]).toEqual(["hello content of z.txt"]);
  expect(screen.fixture.requests.some(request => request.message.SaveBuffer)).toBe(true);
});

test("dirty close modal cancels, saves, and discards via keyboard", async () => {
  const screen = await setup();
  await screen.model.openFile("z.txt");
  await screen.mockInput.typeText("ix"); await screen.escape();
  screen.mockInput.pressKey("w", { ctrl: true }); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Save this buffer before closing?");
  expect(screen.captureCharFrame()).toContain("[S] Save");
  screen.mockInput.pressKey("c"); await screen.settle();
  expect(screen.model.state.confirmation).toBeUndefined();
  expect(screen.model.state.buffers).toHaveLength(1);
  screen.mockInput.pressKey("w", { ctrl: true }); await screen.settle();
  screen.mockInput.pressKey("s");
  await until(() => screen.model.state.buffers.length === 0);
  expect([...screen.fixture.saved.values()]).toEqual(["xcontent of z.txt"]);
  await screen.model.openFile("src/code.rs");
  await screen.mockInput.typeText("iy"); await screen.escape();
  screen.mockInput.pressKey("w", { ctrl: true }); await screen.settle();
  screen.mockInput.pressKey("d"); await screen.settle();
  expect(screen.model.state.buffers).toHaveLength(0);
  expect(screen.fixture.saved.size).toBe(1);
});

test("dirty quit confirmation is distinct and Escape leaves the editor running", async () => {
  const screen = await setup();
  await screen.model.openFile("z.txt");
  await screen.mockInput.typeText("ix"); await screen.escape();
  screen.mockInput.pressKey("q", { ctrl: true }); await screen.settle();
  expect(screen.model.state.confirmation?.kind).toBe("quit");
  expect(screen.captureCharFrame()).toContain("Save modified buffers before quitting?");
  await screen.escape();
  expect(screen.model.state.confirmation).toBeUndefined();
  expect(screen.model.activeBuffer?.editor.frame.dirty).toBe(true);
});

test("Alt+S and Ctrl+P search files, Alt+P and F1 open filterable commands", async () => {
  const screen = await setup();
  for (const shortcut of [{ key: "s", meta: true }, { key: "p", ctrl: true }]) {
    screen.mockInput.pressKey(shortcut.key, shortcut); await screen.settle();
    expect(screen.captureCharFrame()).toContain("Search files");
    expect(screen.captureCharFrame()).not.toContain("Reconnect to daemon");
    await screen.escape();
  }
  for (const shortcut of [{ key: "p", meta: true }, { key: "F1" }]) {
    screen.mockInput.pressKey(shortcut.key, shortcut); await screen.settle();
    expect(screen.captureCharFrame()).toContain("Save buffer");
    await screen.mockInput.typeText("theme"); await screen.settle();
    expect(screen.captureCharFrame()).not.toContain("Save buffer");
    screen.mockInput.pressEnter(); await screen.settle();
    expect(screen.captureCharFrame()).toContain("Themes");
    await screen.escape();
  }
});

test("theme picker persists to shared isolated config and respects configured bindings", async () => {
  const screen = await setup(120, 40, { config: { appearance: { theme: "pastel_dark" }, editor: { "vim-mode": true }, keybinds: { keys: [{ key: "t", with: "Control", action: "OpenCommandPalette" }, { key: "p", with: "Alt", action: "None" }] } } });
  screen.mockInput.pressKey("p", { meta: true }); await screen.settle();
  expect(screen.captureCharFrame()).not.toContain("Reconnect to daemon");
  screen.mockInput.pressKey("t", { ctrl: true }); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Save buffer");
  await screen.mockInput.typeText("theme"); await screen.settle();
  screen.mockInput.pressEnter(); await screen.settle();
  expect(screen.captureCharFrame()).toContain("Themes");
  const nextTheme = Object.keys(palettes).find(name => name !== "pastel_dark")!;
  await screen.mockInput.typeText(nextTheme); await screen.settle();
  screen.mockInput.pressEnter(); await screen.settle();
  expect(screen.model.config.themeName).toBe(nextTheme);
  const persisted = JSON.parse(readFileSync(join(screen.configHome, "config.json"), "utf8"));
  expect(persisted.appearance.theme).toBe(nextTheme);
  expect(persisted.keybinds.keys).toHaveLength(2);
});

test("K hover, F12 definition and completion use real Editor WebSocket replies", async () => {
  const screen = await setup();
  await screen.model.openFile("src/code.rs"); await screen.settle();
  await screen.mockInput.typeText("K");
  await until(() => screen.fixture.requests.some(request => request.message.LspQueryAt?.action === "hover"));
  await screen.settle();
  expect(screen.captureCharFrame()).toContain("Fixture hover documentation");
  await screen.escape();
  screen.mockInput.pressKey("F12");
  await until(() => screen.model.activeBuffer?.path === "src/definition.rs", "Definition opened");
  expect(screen.model.activeBuffer?.editor.frame.cursor.col).toBe(3);
  screen.mockInput.pressKey(" ", { ctrl: true }); await screen.settle();
  expect(screen.captureCharFrame()).toContain("fixtureCompletion");
  screen.mockInput.pressEnter(); await screen.settle();
  expect(screen.model.activeBuffer?.editor.text).toContain("fixtureCompletion");
  const queries = screen.fixture.requests.filter(request => request.message.LspQueryAt);
  expect(queries.map(request => request.message.LspQueryAt.action)).toEqual(["hover", "definition", "completion"]);
  expect(queries.every(request => request.family === "Editor" && request.workspace_root === "/fixture/project")).toBe(true);
});

test("themes show a right-hand preview without applying or persisting until Enter", async () => {
  const screen = await setup();
  screen.mockInput.pressKey("p", { meta: true }); await screen.settle();
  await screen.mockInput.typeText("theme"); screen.mockInput.pressEnter(); await screen.settle();
  const node = (id: string) => screen.renderer.root.findDescendantById(id) as BoxRenderable;
  expect(node("theme-picker-preview").x).toBeGreaterThan(node("theme-picker-list").x);
  const initial = readFileSync(join(screen.configHome, "config.json"), "utf8");
  screen.mockInput.pressArrow("down"); await screen.settle();
  const name = Object.keys(palettes)[1]!;
  expect(node("theme-picker-preview").backgroundColor).toEqual(RGBA.fromHex(palettes[name as keyof typeof palettes].bg));
  expect(screen.model.config.themeName).toBe("pastel_dark");
  expect(readFileSync(join(screen.configHome, "config.json"), "utf8")).toBe(initial);
  const row = node("theme-picker-row-2");
  await screen.mockMouse.moveTo(row.x + 3, row.y); await screen.settle();
  expect(node("theme-picker-preview").backgroundColor).toEqual(RGBA.fromHex(palettes[Object.keys(palettes)[2] as keyof typeof palettes].bg));
  expect(screen.model.config.themeName).toBe("pastel_dark");
  await screen.escape();
  expect(screen.model.config.themeName).toBe("pastel_dark");
  expect(readFileSync(join(screen.configHome, "config.json"), "utf8")).toBe(initial);
});

test("file-tree hover and wheel scrolling work without stealing editor focus, including after resize", async () => {
  const screen = await setup(120, 16, { manyFiles: true });
  await screen.model.openFile("file-079.txt"); await screen.settle();
  const node = (id: string) => screen.renderer.root.findDescendantById(id) as BoxRenderable | undefined;
  const hovered = node("file-tree-row-2")!;
  await screen.mockMouse.moveTo(5, hovered.y); await screen.settle();
  expect(node("file-tree-row-2")!.backgroundColor).toEqual(RGBA.fromHex(screen.model.config.theme.hover));
  await screen.mockMouse.moveTo(60, hovered.y); await screen.settle();
  expect(node("file-tree-row-2")!.backgroundColor).toEqual(RGBA.fromHex(screen.model.config.theme.bg));
  for (let i = 0; i < 5; i++) await screen.mockMouse.scroll(5, 5, "down");
  await screen.settle();
  expect(node("file-tree-row-0")).toBeUndefined();
  expect(screen.model.activeBuffer?.path).toBe("file-079.txt");
  screen.mockInput.pressKey("i"); await screen.settle();
  expect(screen.model.activeBuffer?.editor.frame.mode).toBe("Insert");
  await screen.escape();
  for (let i = 0; i < 100; i++) await screen.mockMouse.scroll(5, 5, "down");
  await screen.settle();
  expect(node("file-tree-row-79")).toBeDefined();
  screen.resize(120, 12); await screen.settle();
  expect(node("file-tree-viewport")!.height).toBe(9);
  for (let i = 0; i < 100; i++) await screen.mockMouse.scroll(5, 4, "up");
  await screen.settle();
  expect(node("file-tree-row-0")).toBeDefined();
  screen.mockInput.pressKey("e", { meta: true });
  for (let i = 0; i < 79; i++) screen.mockInput.pressArrow("down");
  await screen.settle();
  expect(node("file-tree-row-79")).toBeDefined();
  expect(node("file-tree-row-0")).toBeUndefined();
});

test("long tree labels never blank or retain their own scroll offset when viewport slots are recycled", async () => {
  const entries = Array.from({ length: 60 }, (_, index) => ({ name: `dir-${String(index).padStart(2, "0")}-neoism-extremely-long-directory-name`, is_dir: true }));
  const screen = await setup(100, 16, { rootEntries: entries });
  const check = () => {
    const frame = screen.captureCharFrame().split("\n");
    const tree = screen.renderer.root.findDescendantById("file-tree")!;
    let count = 0;
    for (let index = 0; index < entries.length; index++) {
      const row = screen.renderer.root.findDescendantById(`file-tree-row-${index}`);
      if (!row) continue;
      count++;
      expect(frame[row.y]!.slice(0, tree.width)).toContain(entries[index]!.name.slice(0, 7));
      const label = row.getChildren()[0] as import("@opentui/core").TextRenderable;
      expect(label.height).toBe(1);
      expect(label.scrollY).toBe(0);
    }
    expect(count).toBeGreaterThan(0);
  };
  check();
  for (let i = 0; i < 35; i++) {
    await screen.mockMouse.moveTo(15, 5);
    await screen.mockMouse.scroll(15, 5, "down");
    await screen.flush(); check();
  }
  screen.resize(80, 12); await screen.flush(); check();
  for (let i = 0; i < 35; i++) {
    await screen.mockMouse.scroll(15, 4, "up");
    await screen.flush(); check();
  }
});

test("GUI Alt-arrow ladder moves code to buffer/workspace tabs and Files, with separate navigation and activation", async () => {
  const screen = await setup();
  await screen.model.openFile("src/code.rs"); await screen.model.openFile("z.txt"); await screen.settle();
  const alt = async (direction: "left" | "right" | "up" | "down") => { screen.mockInput.pressArrow(direction, { meta: true }); await screen.settle(); };
  const label = (id: string) => screen.renderer.root.findDescendantById(id) as import("@opentui/core").TextRenderable;
  await alt("up");
  expect(label("buffer-tab-2-label").fg).toEqual(RGBA.fromHex(screen.model.config.theme.blue));
  screen.mockInput.pressArrow("left"); await screen.settle();
  expect(label("buffer-tab-1-label").fg).toEqual(RGBA.fromHex(screen.model.config.theme.blue));
  expect(screen.model.activeBuffer?.path).toBe("z.txt");
  screen.mockInput.pressEnter(); await screen.settle();
  expect(screen.model.activeBuffer?.path).toBe("src/code.rs");
  await alt("up");
  screen.mockInput.pressArrow("right"); await screen.settle();
  expect(label("workspace-tab-1-label").fg).toEqual(RGBA.fromHex(screen.model.config.theme.blue));
  expect(screen.model.state.activeWorkspace?.id).toBe("work");
  screen.mockInput.pressEnter(); await screen.settle();
  expect(screen.model.state.activeWorkspace?.id).toBe("second");
  await alt("down"); await alt("down"); await alt("left");
  expect((screen.renderer.root.findDescendantById("file-tree") as BoxRenderable).borderColor).toEqual(RGBA.fromHex(screen.model.config.theme.dim));
  await alt("right"); await alt("up"); await alt("left");
  expect((screen.renderer.root.findDescendantById("file-tree") as BoxRenderable).borderColor).toEqual(RGBA.fromHex(screen.model.config.theme.dim));
  const workspace = screen.renderer.root.findDescendantById("workspace-tab-0")!;
  await screen.mockMouse.click(workspace.x + 2, workspace.y); await screen.settle();
  expect(screen.model.state.activeWorkspace?.id).toBe("work");
  expect(label("workspace-tab-0-label").fg).toEqual(RGBA.fromHex(screen.model.config.theme.blue));
  const buffer = screen.renderer.root.findDescendantById("buffer-tab-1")!;
  await screen.mockMouse.click(buffer.x + 2, buffer.y); await screen.settle();
  const text = screen.model.activeBuffer!.editor.text;
  screen.mockInput.pressKey("i"); await screen.settle();
  expect(screen.model.activeBuffer!.editor.text).toBe(text);
  expect(screen.model.activeBuffer!.editor.frame.mode).toBe("Normal");
  await alt("down"); screen.mockInput.pressKey("i"); await screen.settle();
  expect(screen.model.activeBuffer!.editor.frame.mode).toBe("Insert");
});