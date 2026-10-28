import { expect, test } from "bun:test";
import { NativeEditor } from "../src/native-core";
import { TerminalBuffer } from "../src/terminal/buffer";
import { palettes } from "../src/design/generated";
import { cursorPaint, documentPaintRows } from "../src/editor/paint-frame";

test("editor animation retains glyph identities and exact bounded cursor geometry", () => {
  const editor = new NativeEditor("paint.rs", "fn main() { println!(\"hello\"); }\nsecond", true);
  try {
    const before = editor.call("layout", { width: 800, height: 400, cellWidth: 8, rowHeight: 20 });
    const after = editor.call("frame");
    expect(after.lines).toBe(before.lines);
    expect(after.lines[0].runs).toBe(before.lines[0].runs);
    expect(after.totalLines).toBe(2);
    expect(editor.call("key",{key:"d",char:"d"}).pendingKeys).toBe("d");
    const points = [[102.25, 64.5], [111.5, 65], [110.75, 85.25], [101.5, 84]];
    const paint = cursorPaint(points);
    expect(paint.width * paint.height).toBeLessThan(400);
    const restored = paint.points.split(" ").map(point => point.split(",").map(Number)).map(([x,y]) => [x+paint.left,y+paint.top]);
    expect(restored).toEqual(points);
  } finally { editor.dispose(); }
});

test("scrolling one visual row retains existing wrapped glyph hosts at identical viewport positions",()=>{
  const editor=new NativeEditor("wrapped.txt",Array.from({length:50},(_,i)=>`line${i} `+"abc def ".repeat(14)).join("\n"),true);
  try{
    const before=editor.call("layout",{width:220,height:200,cellWidth:8,rowHeight:20});
    const rows=documentPaintRows([],before);
    const snapshot=editor.snapshot();snapshot.scrollY=before.rowHeight;
    const after=editor.restore(snapshot),next=documentPaintRows(rows,after);
    expect(after.scrollY).toBe(before.rowHeight);
    expect(next[0]).toBe(rows[1]);
    for(let i=0;i<next.length;i++)expect(next[i].y-after.scrollY).toBeCloseTo(after.lines[i].y,3);
  }finally{editor.dispose();}
});

test("compact Ghostty runs preserve colors, erase backgrounds, graphemes, marks and scrolling", async () => {
  const packed = new TerminalBuffer(palettes.pastel_dark, 32, 5, "ghostty", "runs");
  const cells = new TerminalBuffer(palettes.pastel_dark, 32, 5, "ghostty", "cells");
  const compare = () => { expect(packed.frame()).toEqual(cells.frame()); expect(packed.selectedText).toBe(cells.selectedText); };
  const feed = async (text: string) => { await packed.write(new TextEncoder().encode(text)); await cells.write(new TextEncoder().encode(text)); compare(); };
  try {
    await feed("plain \x1b[31mred\x1b[0m 界 e\u0301 👩🏽‍💻\r\n");
    await feed("\x1b[1;3;4;7mstyled\x1b[0m \x1b[8mhidden\x1b[0m\r\n");
    await feed("\x1b[48;2;12;34;56mfill\x1b[K\x1b[0m\r\n");
    await feed("\x1b]4;1;rgb:00/ff/00\x07\x1b[31mnew palette\x1b[0m\r\nlast");
    packed.select(0, 0, true); cells.select(0, 0, true);
    packed.select(1, 7, false); cells.select(1, 7, false); compare();
    for (let i=0; i<6; i++) { packed.scrollPixels(7,20); cells.scrollPixels(7,20); compare(); }
    packed.scrollToBottom(); cells.scrollToBottom(); compare();
    packed.key({key:"space",char:" ",alt:true,shift:true}); cells.key({key:"space",char:" ",alt:true,shift:true});
    for (const buffer of [packed,cells]) { buffer.search("start",{forward:true}); buffer.search("preview",{pattern:"palette"}); }
    compare();
  } finally { packed.dispose(); cells.dispose(); }
});

test("fractional terminal wheel frames keep unchanged text host objects", async () => {
  const terminal = new TerminalBuffer(palettes.pastel_dark, 32, 3);
  try {
    await terminal.write(new TextEncoder().encode("first\r\nsecond\r\nthird\r\nfourth\r\nfifth"));
    terminal.scrollPixels(20,20);
    const before=terminal.frame();
    terminal.scrollPixels(3,20);
    const after=terminal.frame();
    expect(after.offset-before.offset).toBe(3);
    expect(after.lines).toBe(before.lines);
    expect(terminal.frame()).toBe(after);
  } finally { terminal.dispose(); }
});

test("code wheel bursts retain all deltas across keyboard input and cancel on unmount", async () => {
  const editor = new NativeEditor("wheel.rs", Array.from({ length: 200 }, (_, i) => `let value_${i} = ${i};`).join("\n"), true);
  try {
    editor.call("layout", { width: 800, height: 400, cellWidth: 8, rowHeight: 20 });
    const scrolls: number[] = [], call = editor.call.bind(editor);
    editor.call = (operation, args = {}) => {
      if (operation === "scroll") scrolls.push(args.delta as number);
      return call(operation, args);
    };
    editor.queueScroll(-12); editor.queueScroll(-18); editor.queueScroll(-30);
    editor.queueScroll(NaN); editor.queueScroll(0);
    expect(scrolls).toEqual([]);
    editor.call("key", { key: "j", char: "j" });
    const deadline = performance.now() + 500;
    while (!scrolls.length && performance.now() < deadline) await Bun.sleep(5);
    expect(scrolls).toEqual([-60]);
    expect(editor.frame.cursor.line).toBeGreaterThanOrEqual(1);
    expect(editor.frame.dirty).toBe(false);
    editor.queueScroll(-40); editor.cancelFrame();
    await Bun.sleep(15);
    expect(scrolls).toEqual([-60]);
  } finally { editor.dispose(); }
});
