import { expect, test } from "bun:test";
import { loadConfig } from "../src/config";
import { TerminalBuffer } from "../src/terminal/buffer";
import { TerminalBlocks, terminalLinesText } from "../src/terminal/blocks";

const osc = (value: string) => `\x1b]133;${value}\x07`;
const prompt = osc("A") + "$ " + osc("B");
function setup(cols = 40, rows = 5) {
  const buffer = new TerminalBuffer(loadConfig().theme, cols, rows);
  let now = 0;
  const blocks = new TerminalBlocks(buffer, () => now);
  return { buffer, blocks, tick: (ms: number) => { now += ms; },
    write: (text: string, offset?: number) => buffer.write(new TextEncoder().encode(text), offset),
    dispose: () => { blocks.dispose(); buffer.dispose(); } };
}

test("native live snapshots retain ANSI, cursor and identity through command completion", async () => {
  const s = setup();
  try {
    await s.write(prompt);
    expect(s.blocks.prepare("read answer")).toBe(true);
    await s.write("read answer\r\n" + osc("C") + "\x1b[31mAnswer: \x1b[0m");
    const live = s.blocks.snapshot().at(-1)!;
    expect(live.running).toBe(true);
    expect(terminalLinesText(live.lines)).toBe("Answer:");
    expect(live.cursor).toEqual({ row: 0, col: 8, visible: true });
    expect(live.lines.flat().some(run => run.text.includes("Answer:") && run.fg !== loadConfig().theme.fg)).toBe(true);
    expect(s.blocks.snapshot()).toBe(s.blocks.snapshot());
    s.tick(1001);
    const timed = s.blocks.snapshot().at(-1)!;
    expect(timed.duration).toBe(1000);
    expect(timed.lines).toBe(live.lines);
    await s.write("yes\r\n" + osc("D;0") + prompt);
    const done = s.blocks.blocks.at(-1)!;
    expect(done.id).toBe(live.id);
    expect(done.running).toBeUndefined();
    expect(done.cursor).toBeUndefined();
    expect(done.status).toBe(0);
    expect(terminalLinesText(done.lines)).toBe("Answer: yes");
  } finally { s.dispose(); }
});

test("startup and ready-shell asynchronous text survive as non-command records", async () => {
  const s = setup();
  try {
    await s.write("Welcome\r\n" + prompt);
    expect(s.blocks.blocks[0].background).toBe(true);
    expect(s.blocks.blocks[0].command).toBe("");
    expect(terminalLinesText(s.blocks.blocks[0].lines)).toBe("Welcome");
    await s.write("job finished\r\n");
    const live = s.blocks.snapshot().at(-1)!;
    expect(live.background).toBe(true);
    expect(terminalLinesText(live.lines).trim()).toBe("job finished");
    expect(s.blocks.prepare("echo ok")).toBe(true);
    expect(s.blocks.blocks.at(-1)!.id).toBe(live.id);
    await s.write("echo ok\r\n" + osc("C") + "ok\r\n" + osc("D;0") + prompt);
    expect(s.blocks.blocks.map(block => block.background === true)).toEqual([true, true, false]);
    expect(s.blocks.blocks.at(-1)!.command).toBe("echo ok");
  } finally { s.dispose(); }
});

test("snapshots coalesce native captures without moving scroll, selection or input", async () => {
  const s = setup(20, 3);
  try {
    await s.write(prompt); s.blocks.prepare("many");
    await s.write("many\r\n" + osc("C") + "one\r\ntwo\r\nthree\r\nfour\r\n");
    s.buffer.scrollPixels(-40, 20);
    s.buffer.select(0, 0, true); s.buffer.select(0, 2, false);
    const frame = s.buffer.frame(), position = s.buffer.position(), selection = s.buffer.selectedText;
    let captures = 0;
    const capture = s.buffer.capture.bind(s.buffer);
    s.buffer.capture = (...args) => { captures++; return capture(...args); };
    const snapshot = s.blocks.snapshot();
    for (let i = 0; i < 30; i++) expect(s.blocks.snapshot()).toBe(snapshot);
    s.tick(2000); s.blocks.snapshot();
    expect(captures).toBe(1);
    expect(s.buffer.frame()).toBe(frame);
    expect(s.buffer.position()).toEqual(position);
    expect(s.buffer.selectedText).toBe(selection);
    await s.write("five"); s.blocks.snapshot();
    expect(captures).toBe(2);
  } finally { s.dispose(); }
});

test("alternate screen content is never captured into a command record", async () => {
  const s = setup();
  try {
    await s.write(prompt); s.blocks.prepare("interactive");
    await s.write("interactive\r\n" + osc("C") + "before\r\n");
    const live = s.blocks.snapshot().at(-1)!;
    await s.write("\x1b[?1049hsecret alternate contents");
    expect(s.buffer.position().alt).toBe(true);
    expect(terminalLinesText(s.blocks.snapshot().at(-1)!.lines)).toBe("before");
    expect(s.blocks.snapshot().at(-1)!.cursor?.visible).toBe(false);
    await s.write("\x1b[?1049lafter\r\n" + osc("D;0") + prompt);
    const done = s.blocks.blocks.at(-1)!;
    expect(done.id).toBe(live.id);
    expect(terminalLinesText(done.lines)).toBe("before\nafter");
    expect(done.truncated).toBe(false);
  } finally { s.dispose(); }
});

test("native resize tracks the live mark and prompt fallback completes without prompt ink", async () => {
  const s = setup(40, 5);
  try {
    await s.write(prompt); s.blocks.prepare("wide");
    await s.write("wide\r\n" + osc("C") + "abcdefghijklmnopqrstuvwx");
    const id = s.blocks.snapshot().at(-1)!.id;
    s.buffer.resize(12, 5);
    expect(terminalLinesText(s.blocks.snapshot().at(-1)!.lines).replaceAll("\n", "")).toBe("abcdefghijklmnopqrstuvwx");
    await s.write("\r\n" + prompt);
    const done = s.blocks.blocks.at(-1)!;
    expect(done.id).toBe(id);
    expect(done.status).toBeUndefined();
    expect(terminalLinesText(done.lines)).not.toContain("$");
  } finally { s.dispose(); }
});

test("bounded command retention counts removed records truthfully", async () => {
  const s = setup();
  try {
    await s.write(prompt);
    for (let i = 0; i < 205; i++) {
      expect(s.blocks.prepare(`echo ${i}`)).toBe(true);
      await s.write(`echo ${i}\r\n` + osc("C") + `${i}\r\n` + osc("D;0") + prompt);
    }
    expect(s.blocks.blocks.length).toBe(200);
    expect(s.blocks.removed).toBe(5);
    expect(s.blocks.blocks[0].command).toBe("echo 5");
    expect(s.blocks.snapshot().length).toBe(200);
  } finally { s.dispose(); }
});

test("invalid native marks retain only last valid live text and report truncation", async () => {
  const s = setup();
  try {
    await s.write(prompt); s.blocks.prepare("interactive");
    await s.write("interactive\r\n" + osc("C") + "verified text\r\n");
    const live = s.blocks.snapshot().at(-1)!;
    await s.write("\x1bcunrelated after reset");
    const invalid = s.blocks.snapshot().at(-1)!;
    expect(invalid.id).toBe(live.id);
    expect(invalid.lines).toBe(live.lines);
    expect(invalid.truncated).toBe(true);
    expect(invalid.cursor).toBeUndefined();
    await s.write(osc("D;0") + prompt);
    expect(s.blocks.blocks.at(-1)!.truncated).toBe(true);
    expect(terminalLinesText(s.blocks.blocks.at(-1)!.lines)).toBe("verified text");
  } finally { s.dispose(); }
});

async function history(s: ReturnType<typeof setup>) {
  await s.write("Welcome\r\n" + prompt);
  s.blocks.prepare("echo old");
  await s.write("echo old\r\n" + osc("C") + "old output\r\n" + osc("D;0") + prompt);
  await s.write("background job\r\n");
  s.blocks.snapshot();
  s.blocks.view.scroll = 123; s.blocks.view.follow = false; s.blocks.view.horizontal = 4;
  s.blocks.view.collapsed.add(1); s.blocks.view.selected = 1;
}

test("real clear and clear -x bytes clear all native-backed blocks only on successful completion", async () => {
  for (const command of ["clear", "clear -x", "reset"]) {
    const s = setup();
    try {
      // clear's output comes from the installed terminal utility, not a mocked VT.
      const result = command === "reset" ? undefined : Bun.spawnSync(["bash", "--noprofile", "--norc", "-c", command], { env: { ...process.env, TERM: "xterm-256color" } });
      if (result) expect(result.exitCode).toBe(0);
      const output = result ? new TextDecoder().decode(result.stdout) : "\x1bc";
      expect(output.length).toBeGreaterThan(0);
      await history(s);
      const nativeId = s.buffer.id;
      expect(s.blocks.prepare(command)).toBe(true);
      await s.write(command + "\r\n" + osc("C"));
      expect(s.blocks.recentCommands).toEqual(["echo old", command]);
      for (const byte of new TextEncoder().encode(output)) await s.buffer.write(Uint8Array.of(byte));
      expect(s.blocks.blocks.length).toBeGreaterThan(0);
      await s.write(osc("D;0") + prompt);
      expect(s.buffer.id).toBe(nativeId);
      expect(s.blocks.snapshot()).toEqual([]);
      expect(s.blocks.removed).toBe(0);
      expect(s.blocks.view).toEqual({ scroll: 0, follow: true, horizontal: 0, collapsed: new Set(), selected: undefined });
      expect(s.buffer.frame().offset).toBe(0);
      await s.write(prompt);
      expect(s.blocks.snapshot()).toEqual([]);
      s.blocks.dispose();
      const remounted = new TerminalBlocks(s.buffer);
      expect(remounted.snapshot()).toEqual([]);
      await s.write(prompt);
      expect(remounted.snapshot()).toEqual([]);
      remounted.dispose();
    } finally { s.dispose(); }
  }
});

test("failed clear and command-text-only success preserve completed history", async () => {
  for (const [output, status] of [["clear: unavailable\r\n", 1], ["\x1b[H\x1b[2Jerror\r\n", 1], ["\x1bcerror\r\n", 1], ["", 0]] as const) {
    const s = setup();
    try {
      await history(s); s.blocks.prepare("clear");
      await s.write("clear\r\n" + osc("C") + output + osc(`D;${status}`) + prompt);
      expect(s.blocks.blocks.some(block => block.command === "echo old")).toBe(true);
      expect(s.blocks.blocks.at(-1)!.command).toBe("clear");
      expect(s.blocks.blocks.at(-1)!.status).toBe(status);
      if (output.includes("unavailable")) expect(terminalLinesText(s.blocks.blocks.at(-1)!.lines)).toContain("unavailable");
      if (output.includes("error")) expect(terminalLinesText(s.blocks.blocks.at(-1)!.lines)).toContain("error");
    } finally { s.dispose(); }
  }
});

test("native Bash-style typing recognizes clear after visible prompt ink without prepare metadata", async () => {
  for (const command of ["clear", "clear -x", "reset", "/usr/bin/clear", "command clear"]) {
    const s = setup();
    try {
      await history(s);
      await s.write(prompt);
      await s.write(command + "\r\n" + osc("C"));
      expect(s.blocks.running?.command).toBe(command);
      await s.write("\x1b[H\x1b[2J" + osc("D;0") + prompt);
      expect(s.blocks.snapshot()).toEqual([]);
    } finally { s.dispose(); }
  }
});

test("native prompt Ctrl-L and explicit clear discard cached background and reset scroll", async () => {
  for (const native of [false, true]) {
    const s = setup();
    try {
      await history(s);
      if (native) {
        // Readline redraws can contain no new lifecycle markers at all.
        await s.write("\x1b[H\x1b["); await s.write("2J$ ");
      } else s.blocks.clear();
      expect(s.blocks.snapshot()).toEqual([]);
      expect(s.blocks.view.scroll).toBe(0);
      expect(s.blocks.view.follow).toBe(true);
      await s.write(prompt);
      expect(s.blocks.snapshot()).toEqual([]);
      s.blocks.prepare("echo new");
      await s.write("echo new\r\n" + osc("C") + "new\r\n" + osc("D;0") + prompt);
      expect(s.blocks.blocks.length).toBe(1);
      expect(terminalLinesText(s.blocks.blocks[0].lines)).toBe("new");
    } finally { s.dispose(); }
  }
});

test("ordinary application main-screen redraw and alternate-screen clears never erase history", async () => {
  for (const output of ["\x1b[H\x1b[2Jrepaint", "\x1b[3Jrepaint", "\x1bcreset display", "\x1b[?1049h\x1b[H\x1b[2J\x1b[3JTUI\x1b[?1049l"]) {
    const s = setup();
    try {
      await history(s); s.blocks.prepare("interactive");
      await s.write("interactive\r\n" + osc("C") + output + osc("D;0") + prompt);
      expect(s.blocks.blocks.some(block => block.command === "echo old")).toBe(true);
    } finally { s.dispose(); }
  }
});

test("even a clear-named command cannot request UI clearing from the alternate screen", async () => {
  const s = setup();
  try {
    await history(s); s.blocks.prepare("clear");
    await s.write("clear\r\n" + osc("C") + "\x1b[?1049h\x1b[H\x1b[2J\x1b[3J\x1b[?1049l" + osc("D;0") + prompt);
    expect(s.blocks.blocks.some(block => block.command === "echo old")).toBe(true);
  } finally { s.dispose(); }
});

test("observer forwards every fragmented byte to the same native terminal unchanged", async () => {
  const s = setup(), raw = new TerminalBuffer(loadConfig().theme, 40, 5);
  try {
    const bytes = new TextEncoder().encode(prompt + "clear -x\r\n" + osc("C") + "\x1b[H\x1b[2J" + osc("D;0") + prompt + "\x1b[31mUTF-8: λ\x1b[0m\x1b[?1049halt\x1b[2J\x1b[?1049l");
    for (const byte of bytes) {
      await s.buffer.write(Uint8Array.of(byte));
      await raw.write(Uint8Array.of(byte));
    }
    expect(s.buffer.position()).toEqual(raw.position());
    expect(s.buffer.frame()).toEqual(raw.frame());
  } finally { s.dispose(); raw.dispose(); }
});

test("offset replay dedup does not replay clears or split OSC lifecycle events", async () => {
  const s = setup();
  try {
    await history(s); s.blocks.prepare("clear");
    const bytes = new TextEncoder().encode("clear\r\n" + osc("C") + "\x1b[H\x1b[2J\x1b[3J" + osc("D;0") + prompt);
    // Set a continuous offset before streaming one byte and replaying it.
    for (let i = 0; i < bytes.length; i++) {
      await s.buffer.write(bytes.subarray(i, i + 1), i);
      await s.buffer.write(bytes.subarray(i, i + 1), i);
    }
    expect(s.blocks.snapshot()).toEqual([]);
    expect(s.blocks.promptRevision).toBe(3);
    expect(s.blocks.ready).toBe(true);
  } finally { s.dispose(); }
});

test("session offset gaps release live state without joining unrelated output", async () => {
  const s = setup();
  try {
    await s.write(prompt, 0); s.blocks.prepare("old");
    await s.write("old\r\n" + osc("C") + "old output\r\n");
    const oldId = s.blocks.snapshot().at(-1)!.id;
    await s.write("new startup\r\n" + prompt, 10000);
    expect(s.blocks.running).toBeUndefined();
    expect(s.blocks.snapshot().some(block => block.id === oldId)).toBe(false);
    expect(s.blocks.blocks.at(-1)!.background).toBe(true);
    expect(terminalLinesText(s.blocks.blocks.at(-1)!.lines)).toBe("new startup");
    expect(s.blocks.ready).toBe(true);
  } finally { s.dispose(); }
});