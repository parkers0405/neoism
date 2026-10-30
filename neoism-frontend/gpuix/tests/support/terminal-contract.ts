import { expect, test } from "bun:test";
import { TerminalBuffer } from "../../src/terminal/buffer";
import { palettes } from "../../src/design/generated";
const bytes = (text: string) => new TextEncoder().encode(text);
export function terminalContract() {
test("Ghostty: " + "ANSI colors, wide Unicode, and alternate screens survive native rendering", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 4);
  try {
    await buffer.write(bytes("\x1b[31mred\x1b[0m 界"));
    const line = buffer.frame().lines[0];
    expect(line[0].text).toBe("red"); expect(line[0].fg).toBe(palettes.pastel_dark.red);
    expect(line.map((run) => run.text).join("")).toContain("界");
    expect(buffer.frame().cursor.x).toBe(6);
    await buffer.write(bytes("\x1b[?1049hALT"));
    expect(buffer.frame().lines[0].map((run) => run.text).join("")).toContain("ALT");
    await buffer.write(bytes("\x1b[?1049l"));
    expect(buffer.frame().lines[0].map((run) => run.text).join("")).toContain("red");
  } finally { buffer.dispose(); }
});
test("Ghostty: " + "reconnect backlog overlaps are not painted twice", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 2);
  try {
    await buffer.write(bytes("hello"), 0);
    await buffer.write(bytes("lo world"), 3);
    expect(buffer.frame().lines[0].map((run) => run.text).join("").trimEnd()).toBe("hello world");
    expect(buffer.nextOffset).toBe(11);
  } finally { buffer.dispose(); }
});
test("Ghostty: " + "terminal control keys and application cursor mode", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 2), decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
  try {
    expect(decode(buffer.key({ key: "c", char: "c", ctrl: true }))).toBe("\x03");
    expect(decode(buffer.key({ key: "up" }))).toBe("\x1b[A");
    expect(decode(buffer.key({ key: "space", char: " ", ctrl: true }))).toBe("\x00");
    expect(decode(buffer.key({ key: "x", char: "x", alt: true }))).toBe("\x1bx");
    await buffer.write(bytes("\x1b[?1h"));
    expect(decode(buffer.key({ key: "up" }))).toBe("\x1bOA");
    expect(buffer.key({ key: "up", released: true })).toHaveLength(0);
    await buffer.write(bytes("\x1b[?2004h"));
    expect(decode(buffer.paste("hello\nworld"))).toBe("\x1b[200~hello\nworld\x1b[201~");
  } finally { buffer.dispose(); }
});

test("Ghostty: " + "terminal scrollback, fractional wheel displacement, and selection use the same core", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 3);
  try {
    await buffer.write(bytes("one\r\ntwo\r\nthree\r\nfour"));
    buffer.scrollPixels(13, 20);
    expect(buffer.frame().offset).toBe(13);
    buffer.scrollPixels(14, 20);
    // The 7px left after reaching the top points past the edge; Ghostty drops it
    // (desktop `terminal_edge_rejected`) so the grid never parks beyond the top.
    expect(buffer.frame().offset).toBe(0);
    expect(buffer.frame().lines[0].map((run) => run.text).join("").trimEnd()).toBe("one");
    buffer.select(0, 0, true); buffer.select(0, 2, false);
    expect(buffer.selectedText).toBe("one");
    buffer.scrollToBottom(); expect(buffer.frame().offset).toBe(0);
    expect(buffer.frame().lines[0].map((run) => run.text).join("").trimEnd()).toBe("two");
  } finally { buffer.dispose(); }
});
test("Ghostty: " + "a missing reconnect backlog resets partial ANSI state", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 2);
  try {
    await buffer.write(bytes("old\x1b["), 0);
    await buffer.write(bytes("fresh"), 100);
    expect(buffer.frame().lines[0].map((run) => run.text).join("").trimEnd()).toBe("fresh");
  } finally { buffer.dispose(); }
});

test("Ghostty: " + "terminal Vim navigation and visual yank never send motion keys to the shell", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 3);
  try {
    await buffer.write(bytes("one two\r\nsecond"));
    expect(buffer.key({ key: "space", char: " ", alt: true, shift: true })).toHaveLength(0);
    expect(buffer.viMode).toBe(true);
    expect(buffer.key({ key: "up" })).toHaveLength(0);
    expect(buffer.frame().cursor.y).toBe(0);
    buffer.key({ key: "0", char: "0" }); buffer.key({ key: "v", char: "v" }); buffer.key({ key: "l", char: "l" });
    expect(buffer.selectedText).toBe("on");
    buffer.key({ key: "y", char: "y" }); expect(buffer.pendingClipboard).toBe("on"); expect(buffer.selectedText).toBe("");
    buffer.key({ key: "i", char: "i" }); expect(buffer.viMode).toBe(false);
    expect(new TextDecoder().decode(buffer.key({ key: "x", char: "x" }))).toBe("x");
  } finally { buffer.dispose(); }
});

test("Ghostty: " + "terminal Vim regex search commits n/N and restores the cursor on cancel", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 30, 4);
  try {
    await buffer.write(bytes("hit one\r\nhit two\r\nhit three"));
    buffer.key({ key: "space", char: " ", alt: true, shift: true }); buffer.key({ key: "H", char: "H" }); buffer.key({ key: "0", char: "0" });
    buffer.search("start", { forward: true }); buffer.search("preview", { pattern: "two" }); expect(buffer.frame().cursor.y).toBe(1);
    buffer.search("cancel"); expect(buffer.frame().cursor.y).toBe(0);
    buffer.search("start", { forward: true }); buffer.search("preview", { pattern: "hit" }); buffer.search("commit");
    buffer.key({ key: "n", char: "n" }); expect(buffer.frame().cursor.y).toBe(1);
    buffer.key({ key: "N", char: "N" }); expect(buffer.frame().cursor.y).toBe(0);
    expect(buffer.frame().lines[0].some((run) => run.bg === palettes.pastel_dark.yellow + "4d")).toBe(true);
  } finally { buffer.dispose(); }
});

test("Ghostty: " + "terminal SGR mouse, wheel, and focus reports reuse desktop encoding", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 4), writes: string[] = [];
  const cleanup = buffer.onPtyWrite((data) => writes.push(new TextDecoder().decode(data)));
  try {
    expect(buffer.mouse({ kind: "down", row: 1, col: 4, button: 0 })).toBe(false);
    await buffer.write(bytes("\x1b[?1000h\x1b[?1006h\x1b[?1004h"));
    expect(buffer.mouse({ kind: "down", row: 1, col: 4, button: 0 })).toBe(true);
    expect(writes.pop()).toBe("\x1b[<0;5;2M");
    buffer.mouse({ kind: "up", row: 1, col: 4, button: 0 }); expect(writes.pop()).toBe("\x1b[<0;5;2m");
    expect(buffer.mouse({ kind: "down", row: 1, col: 4, button: 0, shift: true })).toBe(false);
    buffer.mouse({ kind: "wheel", row: 1, col: 4, delta: 1, precise: false, rowHeight: 20 });
    expect(writes.pop()).toBe("\x1b[<64;5;2M".repeat(3));
    buffer.focus(true); expect(writes.pop()).toBe("\x1b[I");
    buffer.focus(false); expect(writes.pop()).toBe("\x1b[O");
  } finally { cleanup(); buffer.dispose(); }
});
test("Ghostty: " + "terminal pointer selection uses the shared 60 percent cell edge and semantic words", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 3);
  try {
    await buffer.write(bytes("hello world"));
    buffer.select(0, 0, true, 1, 10); buffer.select(0, 1, false, 15, 10);
    expect(buffer.selectedText).toBe("h");
    buffer.select(0, 1, false, 17, 10); expect(buffer.selectedText).toBe("he");
    buffer.select(0, 1, true, 15, 10, 2); expect(buffer.selectedText).toBe("hello");
  } finally { buffer.dispose(); }
});

}
