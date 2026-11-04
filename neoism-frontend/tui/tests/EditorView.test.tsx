import { afterEach, expect, spyOn, test } from "bun:test";
import { createSignal } from "solid-js";
import { testRender } from "@opentui/solid";
import { EditorView } from "../src/EditorView";
import { TerminalEditor } from "../src/editor";
import { TuiModel, type EditorBuffer } from "../src/model";
import { loadTuiConfig } from "../src/config";

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0)) await cleanup(); });

async function setup(text = "界\talpha\nsecond\nthird", vim = false) {
  const model = new TuiModel(loadTuiConfig(["--offline"], { NEOISM_CONFIG_HOME: "/nonexistent/editor-view-test" }));
  const editor = new TerminalEditor("/tmp/editor-view-test.rs", text, vim);
  const buffer: EditorBuffer = { id: editor.path, path: editor.path, absolutePath: editor.path, root: "/tmp", editor, saving: false };
  // Isolate the view from daemon I/O while exercising the model notification contract.
  const listeners = new Set<() => void>();
  const subscribe = spyOn(model, "subscribe").mockImplementation(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; });
  const changed = spyOn(model, "changed").mockImplementation(current => { current.refresh(); for (const listener of listeners) listener(); });
  const [size, setSize] = createSignal({ width: 24, height: 4 });
  const [focused, setFocused] = createSignal(true);
  const screen = await testRender(() => <EditorView buffer={buffer} model={model} focused={focused()} width={size().width} height={size().height} />, { width: 30, height: 8 });
  cleanups.push(async () => { await screen.dispose(); editor.dispose(); subscribe.mockRestore(); changed.mockRestore(); });
  await screen.flush();
  return { ...screen, model, editor, changed, setSize, setFocused, listeners };
}

test("renders native source rows with aligned line numbers and expanded tabs", async () => {
  const screen = await setup();
  screen.setFocused(false);
  await screen.flush();
  expect(screen.captureCharFrame()).toContain("1 界  alpha");
  expect(screen.captureCharFrame()).toContain("2 second");
  expect(screen.captureCharFrame()).toContain("3 third");
  expect(screen.editor.width).toBe(21);
  expect(screen.editor.height).toBe(4);
  expect(screen.changed).toHaveBeenCalledTimes(1);
});

test("model notifications repaint mutable editors and cursor runs use Unicode/tab cells", async () => {
  const screen = await setup();
  screen.editor.goto(0, Buffer.byteLength("界\t"));
  screen.model.changed(screen.editor);
  await screen.flush();
  const runs = screen.editor.rows(true)[0]!.runs;
  expect(runs.find(run => run.cursor)?.text).toBe("a");
  expect(screen.captureCharFrame().split("\n")[0]).toContain("界  alpha");
  expect(screen.renderer.getCursorState()).toMatchObject({ x: 8, y: 1, visible: true });
  screen.editor.paste("Z");
  screen.model.changed(screen.editor);
  await screen.flush();
  expect(screen.captureCharFrame()).toContain("Zalpha");
  screen.setFocused(false);
  await screen.flush();
  expect(screen.captureCharFrame()).toContain("Zalpha");
  expect(screen.captureCharFrame()).not.toContain("▏");
});

test("resize lays out once and unchanged notifications do not loop", async () => {
  const screen = await setup();
  screen.setSize({ width: 15, height: 2 });
  await screen.flush();
  expect(screen.editor.width).toBe(12);
  expect(screen.editor.height).toBe(2);
  const calls = screen.changed.mock.calls.length;
  screen.model.changed(screen.editor);
  await screen.flush();
  expect(screen.changed.mock.calls.length).toBe(calls + 1);
  expect(screen.captureCharFrame()).not.toContain("3 third");
});

test("mouse click maps terminal cells to native bytes and wheel scrolls integer rows", async () => {
  const screen = await setup("界\talpha\nsecond\nthird\nfourth\nfifth\nsixth");
  await screen.mockMouse.click(7, 0);
  await screen.flush();
  expect(screen.editor.frame.cursor.col).toBe(Buffer.byteLength("界\t"));
  await screen.mockMouse.scroll(8, 1, "down");
  await screen.flush();
  expect(screen.editor.top).toBeGreaterThan(0);
  expect(screen.captureCharFrame()).toContain("second");
  expect(screen.model.state.error).toBeUndefined();
});

test("pointer errors are reported and subscriptions are cleaned up", async () => {
  const screen = await setup();
  const pointer = spyOn(screen.editor, "pointer").mockImplementation(() => { throw new Error("pointer failure"); });
  await screen.mockMouse.click(7, 0);
  await screen.flush();
  expect(screen.model.state.error).toBe("pointer failure");
  pointer.mockRestore();
  expect(screen.listeners.size).toBe(1);
  await screen.dispose();
  expect(screen.listeners.size).toBe(0);
});