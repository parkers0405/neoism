import { afterEach, expect, mock, spyOn, test } from "bun:test";
import { RGBA } from "@opentui/core";
import { createSignal } from "solid-js";
import { testRender } from "@opentui/solid";
import { StatusBar } from "../src/StatusBar";
import { fileIcon, icons } from "../src/icons";
import { palettes } from "../../gpuix/src/design/generated";
import { modelFixture, until } from "./support/model-fixture";

const disposals: Array<() => void | Promise<void>> = [];
afterEach(async () => { for (const dispose of disposals.splice(0).reverse()) await dispose(); });

async function setup(open = true) {
  const fixture = modelFixture();
  disposals.push(() => fixture.dispose());
  const model = fixture.model;
  await until(() => !!model.state.directories[""]);
  if (open) await model.openFile("src/code.rs");
  const [width, setWidth] = createSignal(180);
  const screen = await testRender(() => <StatusBar model={model} width={width()} />, { width: 180, height: 2 });
  disposals.push(() => screen.dispose());
  await screen.flush();
  return { ...screen, model, setWidth, editor: model.activeBuffer?.editor };
}

test("real model renders filled mode, shared file icon, root and one-based ruler without invented services", async () => {
  const screen = await setup();
  const frame = screen.captureCharFrame();
  expect(frame).toContain(` ${icons.mode} NORMAL `);
  expect(frame.split("\n")[1]).toStartWith(`\ue0b6 ${icons.mode} NORMAL \ue0b4 /fixture/project  ${icons.folder} \ue0b4`);
  expect(frame).toContain(fileIcon("code.rs").glyph);
  expect(frame).toContain("src/code.rs");
  expect(frame).toContain("/fixture/project");
  expect(frame).toContain("1:1");
  expect(frame).toContain("100%");
  expect(frame).not.toContain(icons.lsp);
  expect(frame).not.toContain(icons.branch);
  expect(frame).not.toMatch(/connected|running|Ctrl|0\.7\.|FPS|60fps/i);
  expect(frame.split("\n")[0]).toBe("─".repeat(180));
  const border = screen.captureSpans().lines[0]!.spans[0]!;
  expect(border.fg.equals(RGBA.fromHex(screen.model.config.theme.border))).toBe(true);
  const node = (id: string) => screen.renderer.root.findDescendantById(id)!;
  expect(node("status-bar").height).toBe(2);
  expect(node("status-border").height).toBe(1);
  expect(node("status-content").height).toBe(1);
  expect(node("status-content").y).toBe(node("status-border").y + 1);
  const mode = screen.captureSpans().lines[1]!.spans.find(span => span.text.includes("NORMAL"))!;
  expect(mode.bg.equals(RGBA.fromHex(screen.model.config.theme.blue))).toBe(true);
  expect(mode.fg.equals(RGBA.fromHex(screen.model.config.theme.black))).toBe(true);
  const spans = screen.captureSpans().lines[1]!.spans;
  for (const [glyph, color] of [[icons.folder, "red"], ["≡", "green"]] as const) {
    const icon = spans.find(span => span.text.includes(glyph))!;
    expect(icon.bg.equals(RGBA.fromHex(screen.model.config.theme[color]))).toBe(true);
    expect(icon.fg.equals(RGBA.fromHex(screen.model.config.theme.black))).toBe(true);
  }
  for (const label of ["/fixture/project", "1:1", "src/code.rs"]) {
    expect(spans.find(span => span.text.includes(label))!.bg.equals(RGBA.fromHex(screen.model.config.theme.surface))).toBe(true);
  }
  const gap = screen.captureSpans().lines[1]!.spans.find(span => /^ {5,}$/.test(span.text))!;
  expect(gap.bg.equals(RGBA.fromHex("#000000"))).toBe(true);
});

test("editor notifications update mode, dirty and cursor without parent rerender", async () => {
  const screen = await setup();
  const editor = screen.editor!;
  editor.call("key", { key: "i", char: "i" });
  editor.paste("hello\nworld");
  screen.model.changed(editor);
  await screen.flush();
  const frame = screen.captureCharFrame();
  expect(frame).toContain("INSERT");
  expect(frame).toContain("●");
  expect(frame).toContain(`${editor.frame.cursor.line + 1}:${editor.frame.cursor.col + 1}`);
  const mode = screen.captureSpans().lines[1]!.spans.find(span => span.text.includes("INSERT"))!;
  expect(mode.bg.equals(RGBA.fromHex(screen.model.config.theme.magenta))).toBe(true);
});

test("screenshot's AGENT cwd chain uses pink surface text and folder tail, not reversed sections", async () => {
  const screen = await setup();
  const editor = screen.editor!, frame = editor.frame;
  // Model has no agent surface yet: inject only that authoritative mode fact,
  // never infer AGENT merely from an open socket or workspace.
  Object.defineProperty(editor, "frame", { configurable: true, get: () => ({ ...frame, mode: "agent" }) });
  try {
    screen.model.changed(editor); await screen.flush();
    const row = screen.captureCharFrame().split("\n")[1]!;
    expect(row).toStartWith(`\ue0b6 ${icons.mode} AGENT \ue0b4 /fixture/project  ${icons.folder} \ue0b4`);
    const palette = screen.model.config.theme;
    const spans = screen.captureSpans().lines[1]!.spans;
    const mode = spans.find(span => span.text.includes("AGENT"))!;
    expect(mode.fg.equals(RGBA.fromHex(palette.black))).toBe(true);
    expect(mode.bg.equals(RGBA.fromHex(palette.red))).toBe(true);
    const cwd = spans.find(span => span.text.includes("/fixture/project"))!;
    expect(cwd.fg.equals(RGBA.fromHex(palette.red))).toBe(true);
    expect(cwd.bg.equals(RGBA.fromHex(palette.surface))).toBe(true);
    const folder = spans.find(span => span.text.includes(icons.folder))!;
    expect(folder.fg.equals(RGBA.fromHex(palette.black))).toBe(true);
    expect(folder.bg.equals(RGBA.fromHex(palette.red))).toBe(true);
    const caps = spans.filter(span => /[\ue0b6\ue0b4]/.test(span.text));
    expect(caps[0]!.fg.equals(RGBA.fromHex(palette.red))).toBe(true);
    expect(caps[0]!.bg.equals(RGBA.fromHex(palette.bg))).toBe(true);
    // Mode's tail overlaps the dark cwd surface; folder's outer edge ends
    // on the strip. Neither join is a diagonal Powerline arrow.
    expect(caps[1]!.bg.equals(RGBA.fromHex(palette.surface))).toBe(true);
    expect(caps[2]!.fg.equals(RGBA.fromHex(palette.red))).toBe(true);
    expect(caps[2]!.bg.equals(RGBA.fromHex(palette.bg))).toBe(true);
    expect(row).not.toMatch(/[\ue0b0\ue0b2]|FPS/i);
  } finally { delete (editor as unknown as { frame?: unknown }).frame; }
});

test("actual diagnostics, language, LSP states and colored Git diffs are optional facts", async () => {
  const screen = await setup();
  const buffer = screen.model.activeBuffer!;
  buffer.language = "rust"; buffer.lspStatus = "Initializing"; buffer.lspName = "rust-analyzer";
  const item = { message: "issue", source: null, line: 0, col: 0, end_line: 0, end_col: 1, lnum: 1 };
  buffer.diagnostics = [{ ...item, severity: "Error" }, { ...item, severity: "Warn" }];
  screen.model.state.git = { branch: "main", added: 12, deleted: 3 };
  screen.model.changed(buffer.editor);
  await screen.flush();
  const frame = screen.captureCharFrame();
  expect(frame).toContain(`${icons.error} 1`);
  expect(frame).toContain(`${icons.warning} 1`);
  expect(frame).toContain("rust-analyzer");
  expect(frame).toContain("rust");
  expect(frame).toContain("main +12 -3");
  expect(frame).toContain(`\ue0b6 ${icons.branch}  main +12 -3 `);
  // A rounded left icon edge, then a straight join into branch/diff text.
  expect(frame).not.toContain(`${icons.branch} \ue0b4`);
  expect(frame).not.toContain("-3 \ue0b4");
  const spans = screen.captureSpans().lines[1]!.spans;
  const branch = spans.findIndex(span => span.text.includes(icons.branch));
  expect(spans[branch - 1]!.text).toBe("\ue0b6");
  expect(spans[branch - 1]!.fg.equals(RGBA.fromHex(screen.model.config.theme.blue))).toBe(true);
  expect(spans[branch]!.bg.equals(RGBA.fromHex(screen.model.config.theme.blue))).toBe(true);
  expect(spans[branch + 1]!.text).toContain("main");
  expect(spans[branch + 1]!.bg.equals(RGBA.fromHex(screen.model.config.theme.surface))).toBe(true);
  expect(spans.find(span => span.text.includes(icons.error))!.bg.equals(RGBA.fromHex(screen.model.config.theme.red))).toBe(true);
  expect(spans.find(span => span.text.includes(icons.warning))!.bg.equals(RGBA.fromHex(screen.model.config.theme.yellow))).toBe(true);
  expect(spans.find(span => span.text.includes("+12"))!.fg.equals(RGBA.fromHex(screen.model.config.theme.green))).toBe(true);
  expect(spans.find(span => span.text.includes("-3"))!.fg.equals(RGBA.fromHex(screen.model.config.theme.red))).toBe(true);
  for (const [status, color] of [["Initializing", "yellow"], ["Active", "green"], ["Missing", "red"]] as const) {
    buffer.lspStatus = status; screen.model.changed(buffer.editor); await screen.flush();
    const lsp = screen.captureSpans().lines[1]!.spans.find(span => span.text.includes(icons.lsp))!;
    expect(lsp.bg.equals(RGBA.fromHex(screen.model.config.theme[color]))).toBe(true);
    const body = screen.captureSpans().lines[1]!.spans.find(span => span.text.includes("rust-analyzer"))!;
    expect(body.fg.equals(RGBA.fromHex(screen.model.config.theme[color]))).toBe(true);
    expect(body.bg.equals(RGBA.fromHex(screen.model.config.theme.surface))).toBe(true);
  }
  buffer.lspStatus = "unknown";
  screen.model.changed(buffer.editor); await screen.flush();
  expect(screen.captureCharFrame()).not.toContain(icons.lsp);
});

test("adaptive widths keep a full-width rule and truthful ruler on the single content row", async () => {
  const screen = await setup();
  screen.model.state.git = { branch: "very-long-branch-name", added: 999 };
  screen.model.changed(screen.editor!);
  for (const width of [60, 30, 14, 8, 3, 1, 0]) {
    screen.setWidth(width); screen.resize(Math.max(1, width), 3); await screen.flush();
    const rows = screen.captureCharFrame().split("\n");
    expect(rows[0]?.trim()).toBe("─".repeat(width));
    if (width >= 3) expect(rows[1]).toContain("1:1");
    else expect(rows[1]).not.toContain(":");
    expect(rows[2]?.trim()).toBe("");
    // Narrow fallback remains budgeted with caps counted as real cells.
    if (width >= 30) expect(rows[1]).toContain("\ue0b6");
    if (width <= 30) expect(rows[1]).not.toContain("very-long-branch-name");
  }
});

test("multi-digit coordinates are complete or omitted, never clipped to a false position", async () => {
  const screen = await setup();
  const editor = screen.editor!;
  editor.call("key", { key: "i", char: "i" });
  editor.paste("line\n".repeat(123) + "x".repeat(45));
  screen.model.changed(editor);
  const coordinate = `${editor.frame.cursor.line + 1}:${editor.frame.cursor.col + 1}`;
  expect(coordinate.length).toBeGreaterThan(3);
  for (const width of [30, coordinate.length, coordinate.length - 1, 1]) {
    screen.setWidth(width); screen.resize(width, 3); await screen.flush();
    const rows = screen.captureCharFrame().split("\n");
    expect(rows[0]).toBe("─".repeat(width));
    if (width >= coordinate.length) expect(rows[1]).toContain(coordinate);
    else expect(rows[1]).not.toContain(":");
    expect(rows[2]?.trim()).toBe("");
  }
});

test("theme replacement and subscription cleanup are owned by the component", async () => {
  const screen = await setup();
  const before = screen.captureSpans();
  expect(screen.model.config.theme.bg).toBe("#000000");
  screen.model.config.theme = palettes.nvchad_one;
  screen.model.changed(screen.editor!); await screen.flush();
  expect(screen.captureSpans()).not.toEqual(before);
  const unsubscribe = spyOn(screen.model, "subscribe");
  // A separately mounted status line must invoke its own returned disposer.
  const cleanup = mock(() => {});
  unsubscribe.mockImplementation(() => cleanup);
  const second = await testRender(() => <StatusBar model={screen.model} width={30} />, { width: 30, height: 2 });
  await second.flush(); await second.dispose();
  expect(cleanup).toHaveBeenCalledTimes(1);
  unsubscribe.mockRestore();
});

test("workspace-only status does not fabricate an editor mode, ruler or LSP", async () => {
  const screen = await setup(false);
  expect(screen.captureCharFrame()).toContain("/fixture/project");
  expect(screen.captureCharFrame()).not.toMatch(/NORMAL|1:1|100%/);
  expect(screen.captureCharFrame()).not.toContain(icons.lsp);
});