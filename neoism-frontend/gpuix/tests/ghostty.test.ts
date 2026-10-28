import { terminalContract } from "./support/terminal-contract";
terminalContract("ghostty");

import { expect, test } from "bun:test";
import { TerminalBuffer } from "../src/terminal/buffer";
import { palettes } from "../src/design/generated";
const bytes = (text: string) => new TextEncoder().encode(text);
const text = (buffer: TerminalBuffer, row = 0) => buffer.frame().lines[row].map((run) => run.text).join("").trimEnd();
test("Ghostty is the default terminal engine and keeps combining graphemes and VT replies", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 30, 4), writes: string[] = [];
  const cleanup = buffer.onPtyWrite((data) => writes.push(new TextDecoder().decode(data)));
  try {
    expect(buffer.backend).toBe("ghostty");
    await buffer.write(bytes("e\u0301 👩🏽‍💻\x1b[6n"));
    expect(text(buffer)).toBe("e\u0301 👩🏽‍💻"); expect(buffer.frame().cursor.x).toBe(4);
    expect(writes.join("")).toContain("\x1b[1;5R");
  } finally { cleanup(); buffer.dispose(); }
});
test("Ghostty honors the Neoism ANSI palette and application OSC color overrides", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 3);
  try {
    await buffer.write(bytes("\x1b[31;41mR"));
    expect(buffer.frame().lines[0][0].fg).toBe(palettes.pastel_dark.red);
    expect(buffer.frame().lines[0][0].bg).toBe(palettes.pastel_dark.red);
    await buffer.write(bytes("\x1b]4;1;rgb:00/ff/00\x07"));
    expect(buffer.frame().lines[0][0].fg).toBe("#00ff00");
    expect(buffer.frame().lines[0][0].bg).toBe("#00ff00");
  } finally { buffer.dispose(); }
});
test("Ghostty preserves selection anchors through output and supports actual Vim regular expressions", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 30, 4);
  try {
    await buffer.write(bytes("one two\r\nhit 12\r\nhit 99"));
    buffer.select(0, 0, true, 1, 10); await buffer.write(bytes(" more"));
    buffer.select(0, 3, false, 39, 10); expect(buffer.selectedText).toBe("one");
    buffer.key({ key: "space", char: " ", alt: true, shift: true });
    buffer.key({ key: "H", char: "H" }); buffer.key({ key: "0", char: "0" });
    buffer.search("start", { forward: true }); buffer.search("preview", { pattern: "hit [0-9]+" });
    expect(buffer.frame().cursor.y).toBe(1); buffer.search("commit"); buffer.key({ key: "n", char: "n" });
    expect(buffer.frame().cursor.y).toBe(2);
  } finally { buffer.dispose(); }
});

test("Ghostty Vim matches Neoism word, line, viewport, bracket, wide-cell, and wrap motions", async () => {
  const scenarios: Array<{ content: string; cols: number; rows: number; keys: string[] }> = [
    ...["H0w", "H0ww", "H0e", "H0$", "H0$$", "H0lb", "H0M", "H0L", "H0j$", "H0jw", "H0jW", "H0jllkj", "H0ww%", "H0G", "H0g"].map((keys) => ({ content: "one two (four)\r\nsecond\r\n  third line", cols: 30, rows: 5, keys: [...keys] })),
    ...["H0l", "H0ll", "H0llh", "H0llll", "H0llllllllll", "H0j0", "H0j00", "H0j0h"].map((keys) => ({ content: "a界defghijklmnopq\r\nsecond", cols: 10, rows: 4, keys: [...keys] })),
  ];
  for (const scenario of scenarios) {
    const cursors = [];
    for (const backend of ["crosswords", "ghostty"] as const) {
      const buffer = new TerminalBuffer(palettes.pastel_dark, scenario.cols, scenario.rows, backend);
      try {
        await buffer.write(bytes(scenario.content)); buffer.key({ key: "space", char: " ", alt: true, shift: true });
        for (const char of scenario.keys) buffer.key({ key: char.toLowerCase(), char, shift: char !== char.toLowerCase() });
        cursors.push(buffer.frame().cursor);
      } finally { buffer.dispose(); }
    }
    expect({ scenario, cursor: cursors[1] }).toEqual({ scenario, cursor: cursors[0] });
  }
});
test("Ghostty answers TUI device, size and color-scheme queries through the PTY effect", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 30, 4), writes: string[] = [];
  const cleanup = buffer.onPtyWrite((data) => writes.push(new TextDecoder().decode(data)));
  try {
    buffer.resize(31, 5, 12, 24);
    await buffer.write(bytes("\x1b[c\x1b[>c\x1b[18t\x1b[16t\x1b[?996n"));
    const reply = writes.join("");
    expect(reply).toContain("\x1b[?62;22c"); expect(reply).toContain("\x1b[>1;10;0c");
    expect(reply).toContain("\x1b[8;5;31t"); expect(reply).toContain("\x1b[6;24;12t"); expect(reply).toContain("\x1b[?997;1n");
  } finally { cleanup(); buffer.dispose(); }
});
test("Ghostty applies and resets OSC foreground/background overrides without losing the configured theme", async () => {
  const buffer = new TerminalBuffer(palettes.pastel_dark, 20, 3);
  try {
    await buffer.write(bytes("\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:01/02/03\x07plain"));
    expect(buffer.frame().lines[0][0].fg).toBe("#123456"); expect(buffer.frame().lines[0][0].bg).toBe("#010203");
    await buffer.write(bytes("\x1b]110\x07\x1b]111\x07"));
    expect(buffer.frame().lines[0][0].fg).toBe(palettes.pastel_dark.fg); expect(buffer.frame().lines[0][0].bg).toBeUndefined();
  } finally { buffer.dispose(); }
});
