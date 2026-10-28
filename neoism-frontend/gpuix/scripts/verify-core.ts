import assert from "node:assert/strict";
import { NativeEditor, coreCall } from "../src/native-core";
import { TerminalBuffer } from "../src/terminal/buffer";
import { palettes } from "../src/design/generated";

// The caller supplies NEOISM_GPUIX_CORE before this process imports the FFI module.
// Exercise only in-memory engine operations: no files, windows, daemon, or real PTY.
const original = "alpha beta\nsecond\n";
const editor = new NativeEditor("neoism-core-smoke.txt", original, false);
const restored = new NativeEditor("neoism-core-recovery-smoke.txt", "", false);
const vim = new NativeEditor("neoism-core-vim-smoke.txt", "one two", true);
try {
  editor.call("layout", { width: 600, height: 210, cellWidth: 10, rowHeight: 21 });
  editor.call("paste", { text: "é" });
  assert.equal(editor.text(), `é${original}`);
  restored.restore(editor.snapshot());
  assert.equal(restored.text(), editor.text());
  restored.call("key", { key: "z", ctrl: true });
  assert.equal(restored.text(), original);
  for (const key of "dw") vim.call("key", { key, char: key });
  assert.equal(vim.text(), "two");
  vim.call("key", { key: "u", char: "u" });
  assert.equal(vim.text(), "one two");
  assert.equal(coreCall<{ blink: boolean }>("cursor", { seconds: 0.1 }).blink, true);
} finally {
  editor.dispose(); restored.dispose(); vim.dispose();
}

const terminal = new TerminalBuffer(palettes.pastel_dark, 24, 3);
const replies: number[] = [];
const unsubscribe = terminal.onPtyWrite(bytes => replies.push(...bytes));
try {
  await terminal.write(new TextEncoder().encode("\x1b[31mred\x1b[0m e\u0301\x1b[6n\x1b[?2004h"));
  const frame = terminal.frame();
  assert.equal(frame.lines[0].map(run => run.text).join("").trimEnd(), "red e\u0301");
  assert.equal(frame.lines[0][0].fg, palettes.pastel_dark.red);
  assert.ok(new TextDecoder().decode(Uint8Array.from(replies)).includes("\x1b[1;6R"));
  assert.equal(new TextDecoder().decode(terminal.paste("x")), "\x1b[200~x\x1b[201~");
  assert.equal(new TextDecoder().decode(terminal.key({ key: "up" })), "\x1b[A");
} finally {
  unsubscribe(); terminal.dispose();
}
console.log("Native FFI smoke passed: editor, recovery, undo, Vim, cursor, Ghostty, VT replies, and keyboard.");
