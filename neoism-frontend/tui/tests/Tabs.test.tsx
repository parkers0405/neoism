import { afterEach, expect, test } from "bun:test";
import { createSignal } from "solid-js";
import { testRender } from "@opentui/solid";
import { RGBA, type BoxRenderable, type TextRenderable } from "@opentui/core";
import { BufferTabs, WorkspaceTabs } from "../src/Tabs";
import { fileIcon, icons } from "../src/icons";
import { modelFixture, until } from "./support/model-fixture";

const cleanups: Array<() => void | Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0).reverse()) await cleanup(); });

async function setup(width = 100, paths = ["src/main.ts", "lib/main.ts", "lib/code.rs"]) {
  const peer = modelFixture(), model = peer.model;
  cleanups.push(() => peer.dispose());
  await until(() => !!model.state.directories[""]);
  for (const path of paths) {
    const buffer = (await model.openFile(path))!;
    await until(() => model.documents.isSeeded(buffer.editor));
  }
  let focused = 0;
  const [columns, resize] = createSignal(width);
  const screen = await testRender(() => <box flexDirection="column">
    <WorkspaceTabs model={model} width={columns()} onFocusContent={() => focused++} />
    <BufferTabs model={model} width={columns()} onFocusContent={() => focused++} />
    <text id="sentinel" height={1}>Content stays below</text>
  </box>, { width: 120, height: 5 });
  cleanups.push(() => screen.dispose());
  await screen.flush();
  const node = (id: string) => screen.renderer.root.findDescendantById(id)!;
  const click = async (id: string) => { const target = node(id); await screen.mockMouse.click(target.x, target.y); await screen.flush(); };
  return { ...screen, model, node, click, resize, focused: () => focused };
}

test("flat one-row tabs match palette and generated file-icon spans", async () => {
  const s = await setup();
  expect(s.node("workspace-tabs").height).toBe(1);
  expect(s.node("buffer-tabs").height).toBe(1);
  expect(s.node("sentinel").y).toBe(2);
  expect((s.node("workspace-tabs") as BoxRenderable).backgroundColor).toEqual(RGBA.fromHex("#000000"));
  expect((s.node("workspace-tab-0-icon") as TextRenderable).fg).toEqual(RGBA.fromHex("#7ebae4"));
  expect((s.node("buffer-tab-3") as BoxRenderable).backgroundColor).toEqual(RGBA.fromHex(s.model.config.theme.surface));
  expect((s.node("buffer-tab-1") as BoxRenderable).backgroundColor).toEqual(RGBA.fromHex("#000000"));
  expect((s.node("buffer-tab-1-label") as TextRenderable).fg).toEqual(RGBA.fromHex(s.model.config.theme.dim));
  expect((s.node("buffer-tab-3-label") as TextRenderable).fg).toEqual(RGBA.fromHex(s.model.config.theme.fg));
  for (const [index, name] of [[1, "main.ts"], [2, "main.ts"], [3, "code.rs"]] as const) {
    expect((s.node(`buffer-tab-${index}-icon`) as TextRenderable).fg).toEqual(RGBA.fromHex(fileIcon(name).color));
    expect((s.node(`buffer-tab-${index}-icon`) as TextRenderable).plainText).toContain(fileIcon(name).glyph);
  }
  const frame = s.captureCharFrame();
  expect(frame).toContain("src/main.ts"); expect(frame).toContain("lib/main.ts");
  expect(frame).toContain(`${icons.home} Home`);
  expect(frame).not.toMatch(/[\ue0b6\ue0b4]/);
  for (const id of ["workspace-tab-0", "buffer-tab-3", "buffer-tab-1"]) {
    const fill = (s.node(id) as BoxRenderable).backgroundColor;
    for (const edge of ["leading", "trailing"]) {
      const padding = s.node(`${id}-${edge}`) as TextRenderable;
      expect(padding.plainText).toBe(" ");
      expect(padding.bg).toEqual(fill);
    }
  }
  expect(frame).toContain("project"); expect(frame).not.toContain("Neoism");
});

test("buffer selection and Home focus content; dirty close is isolated and protected", async () => {
  const s = await setup();
  await s.click("buffer-tab-1-label");
  s.model.paste("changed "); await s.flush();
  expect(s.node("buffer-tab-1-dirty")).toBeDefined();
  expect((s.node("buffer-tab-1-dirty") as TextRenderable).fg).toEqual(RGBA.fromHex(s.model.config.theme.yellow));
  await s.click("buffer-tab-3-label");
  await s.click("buffer-tab-1-close");
  expect(s.model.state.activePath).toBe("lib/code.rs");
  expect(s.model.state.confirmation?.kind).toBe("close");
  expect(s.model.state.buffers).toHaveLength(3);
  s.model.cancelConfirmation();
  expect(s.model.state.buffers[1]!.editor.frame.dirty).toBe(false);
  await s.flush();
  await s.click("buffer-tab-2-close");
  expect(s.model.state.buffers.map(buffer => buffer.path)).toEqual(["src/main.ts", "lib/code.rs"]);
  expect(s.model.state.activePath).toBe("lib/code.rs");
  await s.click("buffer-tab-0-label");
  expect(s.model.state.activePath).toBeUndefined();
  expect(s.focused()).toBe(5);
});

test("workspace mouse switching retains buffers and selection through model views", async () => {
  const s = await setup();
  const original = s.model.activeBuffer;
  await s.click("workspace-tab-1-label");
  expect(s.model.state.activeWorkspace?.id).toBe("second");
  expect(s.model.state.buffers).toHaveLength(0);
  await s.model.openFile("other.ts"); await s.flush();
  await s.click("workspace-tab-0-label");
  expect(s.model.activeBuffer).toBe(original);
  expect(s.captureCharFrame()).toContain("code.rs");
  await s.click("workspace-tab-1-label");
  expect(s.model.state.activePath).toBe("other.ts");
});

test("active reveal, clickable overflow cycling and unicode resizing never spill", async () => {
  const s = await setup(23, ["first.ts", "e\u0301.ts", "目录/文件.ts", "last.rs"]);
  expect(s.node("buffer-tab-4")).toBeDefined();
  expect(s.renderer.root.findDescendantById("buffer-tab-0")).toBeUndefined();
  await s.click("buffer-next");
  expect(s.model.state.activePath).toBeUndefined();
  expect(s.node("buffer-tab-0")).toBeDefined();
  await s.click("buffer-previous");
  expect(s.model.state.activePath).toBe("last.rs");
  s.model.selectBuffer("目录/文件.ts"); await s.flush();
  expect(s.node("buffer-tab-3")).toBeDefined();
  for (const width of [12, 7, 3, 2, 1, 100]) {
    s.resize(width); await s.flush();
    expect(s.node("buffer-tabs").width, `strip at width=${width}`).toBe(width);
    expect(s.node("buffer-tabs").height).toBe(1);
    expect(s.node("sentinel").y).toBe(2);
    if (width) expect(s.node("buffer-tab-3")).toBeDefined();
    for (const kind of ["workspace", "buffer"]) {
      for (const child of s.node(`${kind}-tabs`).getChildren()) {
        expect(child.x + child.width, `${kind} width=${width} child=${child.id}`).toBeLessThanOrEqual(width);
      }
    }
  }
  expect(s.captureCharFrame()).toContain("文件.ts");
});

test("each strip subscribes independently and disposes its own listener", async () => {
  const peer = modelFixture(); cleanups.push(() => peer.dispose());
  const model = peer.model;
  const subscribe = model.subscribe, listeners = new Set<() => void>();
  model.subscribe = listener => { listeners.add(listener); const off = subscribe(listener); return () => { listeners.delete(listener); off(); }; };
  const [shown, show] = createSignal(true);
  const screen = await testRender(() => <box>{shown() && <><WorkspaceTabs model={model} width={50} /><BufferTabs model={model} width={50} /></>}</box>, { width: 50, height: 4 });
  cleanups.push(() => screen.dispose());
  await screen.flush(); expect(listeners.size).toBe(2);
  show(false); await screen.flush(); expect(listeners.size).toBe(0);
});