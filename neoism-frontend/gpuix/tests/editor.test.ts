import { expect, test } from "bun:test";
import { NativeEditor, coreCall } from "../src/native-core";
const press = (editor: NativeEditor, sequence: string) => { for (const char of sequence) editor.call("key", { key: char, char }); };

test("real Vim resolver handles word operators, undo, insert, and escape", () => {
  const editor = new NativeEditor("example.rs", "alpha beta gamma\nsecond\n", true);
  try {
    press(editor, "wdw"); expect(editor.text()).toBe("alpha gamma\nsecond\n");
    press(editor, "u"); expect(editor.text()).toBe("alpha beta gamma\nsecond\n");
    press(editor, "i"); expect(editor.frame.mode).toBe("Insert"); expect(editor.frame.cursor.shape).toBe("Beam");
    press(editor, "x"); expect(editor.text()).toBe("alpha xbeta gamma\nsecond\n");
    editor.call("key", { key: "escape" }); expect(editor.frame.mode).toBe("Normal"); expect(editor.frame.cursor.shape).toBe("Block");
    expect(editor.frame.dirty).toBe(true);
    editor.call("saved"); expect(editor.frame.dirty).toBe(false);
  } finally { editor.dispose(); }
});
test("native cursor hit testing uses UTF-8 byte columns and visual selection", () => {
  const editor = new NativeEditor("example.txt", "abéfg\nsecond line\n", true);
  try {
    const frame = editor.call("layout", { width: 600, height: 210, cellWidth: 10, rowHeight: 21 });
    editor.call("pointer", { x: frame.textX + 31, y: 5, extend: false });
    expect(editor.frame.cursor.col).toBe(4);
    press(editor, "v"); press(editor, "l"); expect(editor.frame.mode).toBe("Visual");
    expect(editor.frame.selection).not.toBeNull();
  } finally { editor.dispose(); }
});
test("Ctrl-D follows the existing integer-row center lock", () => {
  const editor = new NativeEditor("long.txt", Array.from({ length: 100 }, (_, i) => `line ${i}`).join("\n"), true);
  try {
    editor.call("layout", { width: 600, height: 210, cellWidth: 10, rowHeight: 21 });
    const frame = editor.call("key", { key: "d", ctrl: true });
    expect(frame.cursor.line).toBe(9);
    expect(frame.animating).toBe(true);
    expect(frame.cursorQuad).toHaveLength(4);
    expect(frame.contentHeight).toBe(2100);
  } finally { editor.dispose(); }
});
test("caret blink uses Neoism's shared 500ms period", () => {
  expect(coreCall<{ blink: boolean }>("cursor", { seconds: 0.1 }).blink).toBe(true);
  expect(coreCall<{ blink: boolean }>("cursor", { seconds: 0.3 }).blink).toBe(false);
  expect(coreCall<{ blink: boolean }>("cursor", { seconds: 0.6 }).blink).toBe(true);
});

test("bare Vim motions follow wraps while counts follow source lines", () => {
  const editor = new NativeEditor("wrap.txt", "a".repeat(100) + "\nsecond\nthird", true);
  try {
    editor.call("layout", { width: 200, height: 210, cellWidth: 10, rowHeight: 21 });
    press(editor, "j"); expect(editor.frame.cursor.line).toBe(0); expect(editor.frame.cursor.col).toBeGreaterThan(0);
    press(editor, "gg2j"); expect(editor.frame.cursor.line).toBe(2);
    press(editor, "ggvj"); expect(editor.frame.mode).toBe("Visual"); expect(editor.frame.selection).not.toBeNull();
  } finally { editor.dispose(); }
});
test("Vim macros replay the existing resolver and clipboard registers cross the bridge", () => {
  const editor = new NativeEditor("macro.txt", "one two three four", true);
  try {
    press(editor, "qadwq"); expect(editor.text()).toBe("two three four");
    press(editor, "@a"); expect(editor.text()).toBe("three four");
    editor.call("key", { key: "y", char: "y" });
    const yanked = editor.call("key", { key: "y", char: "y" });
    expect(yanked.clipboard).toBe("three four\n");
    editor.call("key", { key: "p", char: "p", clipboard: "external\n" });
    // The original engine prefers its populated unnamed register.
    expect(editor.text()).toBe("three four\nthree four");
    const fresh = new NativeEditor("external.txt", "first", true);
    try {
      fresh.call("key", { key: "p", char: "p", clipboard: "external\n" });
      expect(fresh.text()).toBe("first\nexternal");
    } finally { fresh.dispose(); }
  } finally { editor.dispose(); }
});
test("standard selection, cut, and paste retain UTF-8 boundaries", () => {
  const editor = new NativeEditor("standard.txt", "héllo\nworld", false);
  try {
    editor.call("key", { key: "a", ctrl: true });
    expect(coreCall<{ text: string }>("editor.cut", { id: editor.id }).text).toBe("héllo\nworld");
    expect(editor.text()).toBe("");
    editor.call("paste", { text: "界\nnew" }); expect(editor.text()).toBe("界\nnew");
    editor.call("key", { key: "z", ctrl: true }); expect(editor.text()).toBe("");
  } finally { editor.dispose(); }
});

test("incremental search restores on cancel and commits direction for n/N", () => {
  const editor = new NativeEditor("search.txt", "start\nhit one\nhit two\nhit three\nend", true);
  try {
    editor.call("search.start");
    editor.call("search.preview", { pattern: "hit" }); expect(editor.frame.cursor.line).toBe(1);
    expect(editor.frame.searchHits).toHaveLength(3);
    editor.call("search.cancel"); expect(editor.frame.cursor.line).toBe(0); expect(editor.frame.searchHits).toHaveLength(0);
    editor.call("search.start"); editor.call("search.preview", { pattern: "hit" }); editor.call("search.commit", { pattern: "hit" });
    press(editor, "n"); expect(editor.frame.cursor.line).toBe(2);
    press(editor, "N"); expect(editor.frame.cursor.line).toBe(1);
    editor.call("search.start", { backward: true }); editor.call("search.preview", { pattern: "hit" }); editor.call("search.commit", { pattern: "hit" });
    press(editor, "n"); expect(editor.frame.cursor.line).toBe(3);
    press(editor, "n"); expect(editor.frame.cursor.line).toBe(2);
  } finally { editor.dispose(); }
});
test("ex commands reuse the shared line, save, and substitute classifiers", () => {
  const editor = new NativeEditor("commands.txt", "foo foo\nfoo", true);
  try {
    editor.call("command", { command: ":$" }); expect(editor.frame.cursor.line).toBe(1);
    editor.call("command", { command: ":1" }); expect(editor.frame.cursor.line).toBe(0);
    editor.call("command", { command: ":%s/foo/bar/g" }); expect(editor.text()).toBe("bar bar\nbar");
    press(editor, "u"); expect(editor.text()).toBe("foo foo\nfoo");
    expect(editor.call("command", { command: ":wq" }).hostAction).toBe("save-close");
    expect(() => editor.call("command", { command: ":unknown" })).toThrow("not ported");
  } finally { editor.dispose(); }
});
test("a save acknowledgment cannot mark newer edits as saved", () => {
  const editor = new NativeEditor("save.txt", "before", false);
  try {
    editor.call("paste", { text: "one" }); const revision = editor.frame.revision;
    editor.call("paste", { text: "two" }); editor.call("saved", { revision });
    expect(editor.frame.dirty).toBe(true);
    editor.call("saved", { revision: editor.frame.revision }); expect(editor.frame.dirty).toBe(false);
  } finally { editor.dispose(); }
});

test("additional editor carets share occurrence selection and text edits", () => {
  const editor = new NativeEditor("carets.txt", "foo foo\nfoo", false);
  try {
    editor.call("key", { key: "d", ctrl: true }); editor.call("key", { key: "d", ctrl: true });
    expect(editor.frame.extraCarets).toHaveLength(1);
    press(editor, "bar"); expect(editor.text()).toBe("bar bar\nfoo");
    editor.call("key", { key: "escape" }); expect(editor.frame.extraCarets).toHaveLength(0);
  } finally { editor.dispose(); }
});
test("Ctrl-Alt arrows add carets before Vim interception and Escape collapses them", () => {
  const editor = new NativeEditor("stacked.txt", "one\ntwo\nthree", true);
  try {
    editor.call("key", { key: "down", ctrl: true, alt: true });
    expect(editor.frame.extraCarets).toHaveLength(1);
    editor.call("key", { key: "escape" }); expect(editor.frame.extraCarets).toHaveLength(0);
  } finally { editor.dispose(); }
});

test("standard arrow navigation also follows visual rows in wrapped text", () => {
  const editor = new NativeEditor("arrows.txt", "a".repeat(100) + "\nsecond", false);
  try {
    editor.call("layout", { width: 200, height: 210, cellWidth: 10, rowHeight: 21 });
    editor.call("key", { key: "down" }); expect(editor.frame.cursor.line).toBe(0); expect(editor.frame.cursor.col).toBeGreaterThan(0);
  } finally { editor.dispose(); }
});

test("editor recovery keeps unsaved text, saved baseline, cursor and both history directions", () => {
  const editor = new NativeEditor("recover.txt", "héllo\r\nsecond\r\n", false);
  const recovered = new NativeEditor("recover.txt", "host file may have changed", false);
  try {
    editor.call("paste", { text: "first" }); editor.call("paste", { text: "second" });
    editor.call("key", { key: "z", ctrl: true });
    editor.call("key", { key: "end" }); editor.call("key", { key: "left", shift: true });
    const snapshot = JSON.parse(JSON.stringify(editor.snapshot())); recovered.restore(snapshot);
    expect(recovered.text()).toBe(editor.text()); expect(recovered.frame.cursor.col).toBe(editor.frame.cursor.col);
    expect(recovered.frame.selection).toEqual(editor.frame.selection); expect(recovered.frame.dirty).toBe(true);
    recovered.call("key", { key: "y", ctrl: true }); expect(recovered.text()).toContain("firstsecond");
    recovered.call("key", { key: "z", ctrl: true }); recovered.call("key", { key: "z", ctrl: true });
    expect(recovered.text()).toBe("héllo\r\nsecond\r\n"); expect(recovered.frame.dirty).toBe(false);
  } finally { editor.dispose(); recovered.dispose(); }
});
test("split editor views synchronize edits and undo while retaining their own cursor and selection", () => {
  const editor = new NativeEditor("views.txt", "one\ntwo\nthree", false), view = editor.cloneView();
  try {
    view.call("key", { key: "end", ctrl: true }); view.call("key", { key: "left", shift: true });
    const cursor = { ...view.frame.cursor }, selection = structuredClone(view.frame.selection);
    editor.call("paste", { text: "new " }); view.syncFrom(editor);
    expect(view.text()).toBe("new one\ntwo\nthree"); expect(view.frame.cursor.line).toBe(cursor.line);
    expect(view.frame.cursor.col).toBe(cursor.col); expect(view.frame.selection).toEqual(selection);
    view.call("key", { key: "z", ctrl: true }); editor.syncFrom(view);
    expect(editor.text()).toBe("one\ntwo\nthree"); expect(editor.frame.dirty).toBe(false);
    view.call("paste", { text: "X" }); editor.syncFrom(view);
    view.call("saved"); editor.syncFrom(view); expect(editor.frame.dirty).toBe(false);
  } finally { editor.dispose(); view.dispose(); }
});
test("double and triple pointer presses use the desktop Unicode word and source-line selection", () => {
  const editor = new NativeEditor("pointer.txt", "one naïve_word (x)\nsecond line", false);
  try {
    const frame = editor.call("layout", { width: 600, height: 210, cellWidth: 10, rowHeight: 21 });
    editor.call("pointer", { x: frame.textX + 70, y: 5, clicks: 2 });
    expect(coreCall<{ text: string }>("editor.copy", { id: editor.id }).text).toBe("naïve_word");
    editor.call("pointer", { x: frame.textX + 150, y: 5, clicks: 2 });
    expect(coreCall<{ text: string }>("editor.copy", { id: editor.id }).text).toBe("(");
    editor.call("pointer", { x: frame.textX + 20, y: frame.rowHeight + 5, clicks: 3 });
    expect(coreCall<{ text: string }>("editor.copy", { id: editor.id }).text).toBe("second line");
    editor.call("pointer-up");
    const previous = structuredClone(editor.frame.selection);
    editor.call("pointer", { x: frame.textX, y: 5, drag: true });
    expect(editor.frame.selection).toEqual(previous);
  } finally { editor.dispose(); }
});
