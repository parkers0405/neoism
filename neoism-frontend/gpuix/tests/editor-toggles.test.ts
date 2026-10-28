import { expect, test } from "bun:test";
import { NativeEditor } from "../src/native-core";

test("word wrap changes shared visual rows without changing document or caret", () => {
  const editor = new NativeEditor("wrap.ts", "const longName = '" + "word ".repeat(80) + "';", true);
  try {
    editor.call("layout", { width: 300, height: 200, cellWidth: 8, rowHeight: 20 });
    const before = editor.text();
    const unwrapped = editor.call("wrap", { enabled: false });
    expect(unwrapped.wrap).toBe(false);
    expect(unwrapped.lines).toHaveLength(1);
    const wrapped = editor.call("wrap");
    expect(wrapped.wrap).toBe(true);
    expect(wrapped.lines.some(line => line.continuation)).toBe(true);
    expect(wrapped.cursor.line).toBe(unwrapped.cursor.line);
    expect(wrapped.cursor.col).toBe(unwrapped.cursor.col);
    expect(editor.text()).toBe(before);
  } finally { editor.dispose(); }
});

test("Vim toggles clear pending modal input and switch actual editing behavior", () => {
  const editor = new NativeEditor("mode.ts", "abc", true);
  try {
    editor.call("key", { key: "d", char: "d" });
    expect(editor.frame.pendingEmpty).toBe(false);
    const standard = editor.call("toggle-vim");
    expect(standard.vim).toBe(false);
    expect(standard.mode).toBe("Insert");
    expect(standard.pendingEmpty).toBe(true);
    editor.call("key", { key: "x", char: "x" });
    expect(editor.text()).toBe("xabc");
    const vim = editor.call("toggle-vim");
    expect(vim.vim).toBe(true);
    expect(vim.mode).toBe("Normal");
    editor.call("key", { key: "x", char: "x" });
    expect(editor.text()).toBe("xbc");
  } finally { editor.dispose(); }
});
