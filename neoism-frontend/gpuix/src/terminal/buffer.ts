import type { Palette } from "../design/generated";
import { coreCall } from "../native-core";
import { ghosttyLibraryPath } from "../native-libraries";

type Color = "Default" | { Named: number } | { Indexed: number } | { Spec: { r: number; g: number; b: number } };
interface Cell { c: string; fg: Color; bg: Color; flags: string }
export type PackedTerminalRun = [string, number, number, number, number, number, number];
export interface TerminalPosition { row: number; col: number; history: number; rows: number; cols: number; alt: boolean }
interface NativeFrame {
  paintRows?: Array<Array<[string, number, number, number, number, number, number]>>;
  /** Row just past the viewport on the side a sub-row wheel residual uncovers. */
  extraRow?: { above: boolean; runs: Array<[string, number, number, number, number, number, number]> } | null;
  snapshot: { cols: number; rows: number; alt_screen?: boolean; viewport: Cell[][]; display_offset: number;
    cursor: { col: number; row: number; visible: boolean; shape: "Block" | "Beam" | "Underline" | "Hidden" } };
  writes: number[]; offset: number; viMode: boolean; selectionBlock: boolean; appCursor: boolean; bracketedPaste: boolean; graphemes: Record<string, string>;
  searchMatches?: { start: number[]; end: number[] }[];
  selectionRows?: { row: number; start: number; end: number }[];
  selection?: string; selectionRange?: { start: number[]; end: number[] };
}
export interface CellRun { text: string; column: number; width: number; fg: string; bg?: string; bold: boolean; italic: boolean; underline: boolean; fgTheme?: "fg" | "bg"; bgTheme?: "fg" }
const terminalPalette = (theme: Palette) => [theme.black, theme.red, theme.green, theme.yellow, theme.blue, theme.magenta, theme.cyan, theme.white,
  theme.muted, theme.red, theme.green, theme.yellow, theme.blue, theme.magenta, theme.cyan, theme.fg];
export interface TerminalCursorFrame {quad:number[][];hold:boolean;animating:boolean}
export interface TerminalFrame {
  lines: readonly (readonly CellRun[])[];
  cursor: { x: number; y: number; visible: boolean; shape: "Block" | "Beam" | "Underline" | "Hidden" };
  cols: number; rows: number; offset: number;
  /** Painted at row -1 (above) or `rows` (below) while scrolled by part of a row. */
  extra?: { above: boolean; line: readonly CellRun[] };
}

// Ghostty owns VT state and input encoding. GPUIX paints its cell snapshots;
// the shared scroll/cursor policy preserves Neoism interaction behavior.
export class TerminalBuffer {
  readonly id: number;
  private reply: NativeFrame;
  private listeners = new Set<() => void>();
  private writeListeners = new Set<(bytes: Uint8Array) => void>();
  private palette: string[];
  private offset?: number;
  private disposed = false;
  private metrics = { cellWidth: 10, rowHeight: 20 };
  private painted?: TerminalFrame;
  private paintedReply?: NativeFrame;
  private outputObserver?: { feed: (bytes: Uint8Array, write: (bytes: Uint8Array) => void) => void; reset: () => void };
  pendingClipboard?: string;
  lastKeyHandled = false;
  get viMode() { return this.reply.viMode; }
  get bindingState() { return { vi: this.reply.viMode, appcursor: this.reply.appCursor, alt: !!this.reply.snapshot.alt_screen }; }
  readonly backend = "ghostty";
  constructor(public theme: Palette, cols = 100, rows = 30, encoding: "runs" | "cells" = "runs") {
    this.palette = terminalPalette(theme);
    coreCall("ghostty.load", { path: ghosttyLibraryPath() });
    this.id = coreCall<{ id: number }>(this.op("new"), { cols, rows, palette: this.palette, foreground: theme.fg, background: theme.bg, compact: encoding === "runs" }).id;
    this.reply = coreCall(this.op("frame"), { id: this.id });
  }
  private op(name: string) { return `ghostty.${name}`; }
  setTheme(theme: Palette) {
    const palette = terminalPalette(theme);
    if (this.disposed || theme.fg === this.theme.fg && theme.bg === this.theme.bg && palette.every((color, index) => color === this.palette[index])) return false;
    this.theme = theme; this.palette = palette; this.paintedReply = undefined;
    this.call("colors", { palette, foreground: theme.fg, background: theme.bg });
    return true;
  }
  cursor(cellWidth: number, rowHeight: number): TerminalCursorFrame {
    if (this.disposed) return { quad: [], hold: false, animating: false };
    return coreCall<TerminalCursorFrame>(this.op("cursor"), { id: this.id, cellWidth, rowHeight });
  }
  get cols() { return this.reply.snapshot.cols; }
  get rows() { return this.reply.snapshot.rows; }
  get selectedText() { return this.reply.selection || ""; }
  subscribe(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  onPtyWrite(listener: (bytes: Uint8Array) => void) { this.writeListeners.add(listener); return () => { this.writeListeners.delete(listener); }; }
  position(): TerminalPosition { return coreCall(this.op("position"), { id: this.id }); }
  mark() { return coreCall<{mark: number}>(this.op("mark"), { id: this.id }).mark; }
  unmark(mark: number) { if (!this.disposed) coreCall(this.op("unmark"), { id: this.id, mark }); }
  capture(start: number, end: number, mark?: number) {
    const result = coreCall<{paintRows: PackedTerminalRun[][]; start: number; end: number; truncated: boolean}>(this.op("capture"), { id: this.id, start, end, ...(mark === undefined ? {} : { mark }) });
    return { ...result, lines: result.paintRows.map(row => row.map(run => this.packedRun(run))) };
  }
  observeOutput(observer: NonNullable<TerminalBuffer["outputObserver"]>) {
    this.outputObserver = observer;
    return () => { if (this.outputObserver === observer) this.outputObserver = undefined; };
  }
  private call(op: string, args: Record<string, unknown> = {}) {
    if (this.disposed) return;
    this.reply = coreCall(this.op(op), { id: this.id, ...args });
    if (this.reply.writes.length) for (const listener of this.writeListeners) listener(Uint8Array.from(this.reply.writes));
    if (!this.disposed) for (const listener of this.listeners) listener();
  }
  async write(bytes: Uint8Array, offset?: number): Promise<void> {
    if (this.disposed) return;
    let data = bytes;
    if (offset !== undefined) {
      if (this.offset !== undefined && offset < this.offset) data = bytes.subarray(Math.min(bytes.length, this.offset - offset));
      if (this.offset !== undefined && offset > this.offset) {
        this.outputObserver?.reset();
        this.call("reset", { palette: this.palette, foreground: this.theme.fg, background: this.theme.bg });
      }
      this.offset = Math.max(this.offset ?? 0, offset + bytes.length);
    }
    if (this.outputObserver) this.outputObserver.feed(data, bytes => this.call("feed", { bytes: [...bytes] }));
    else this.call("feed", { bytes: [...data] });
  }
  resize(cols: number, rows: number, cellWidth = 10, rowHeight = 20) {
    if (this.disposed) return false;
    cols = Math.max(2, Math.min(500, Math.floor(cols))); rows = Math.max(1, Math.min(200, Math.floor(rows)));
    const resized = cols !== this.cols || rows !== this.rows;
    if (!resized && cellWidth === this.metrics.cellWidth && rowHeight === this.metrics.rowHeight) return false;
    this.metrics = { cellWidth, rowHeight };
    this.call("resize", { cols, rows, cellWidth, rowHeight }); return resized;
  }
  scrollPixels(delta: number, rowHeight: number) { this.call("scroll", { delta, rowHeight }); }
  scrollToBottom() { this.call("bottom"); }
  paste(text: string) {
    if (this.disposed) return new Uint8Array();
    return Uint8Array.from(coreCall<{ bytes: number[] }>(this.op("paste"), { id: this.id, text }).bytes);
  }
  search(op: "start" | "preview" | "commit" | "cancel", args: Record<string, unknown> = {}) { this.call(`search.${op}`, args); }
  select(row: number, col: number, start: boolean, x?: number, cellWidth = 10, clicks = 1) { this.call("select", { row, col, start, x: x ?? (col + (start ? 0 : 0.9)) * cellWidth, cellWidth, clicks }); }
  mouse(event: Record<string, unknown>) {
    if (this.disposed) return false;
    const reply = coreCall<{ bytes: number[]; handled: boolean }>(this.op("mouse"), { id: this.id, ...event });
    if (reply.bytes.length) for (const listener of this.writeListeners) listener(Uint8Array.from(reply.bytes));
    return reply.handled;
  }
  focus(focused: boolean) {
    // PTY exit disposes the engine before Solid unmounts its terminal views.
    // Their blur/cleanup callbacks must not call a freed native terminal.
    if (this.disposed) return;
    const reply = coreCall<{ bytes: number[] }>(this.op("focus"), { id: this.id, focused });
    if (reply.bytes.length) for (const listener of this.writeListeners) listener(Uint8Array.from(reply.bytes));
  }
  key(event: Record<string, unknown>) {
    if (this.disposed) { this.lastKeyHandled = false; return new Uint8Array(); }
    const reply = coreCall<{ bytes: number[]; clipboard?: string; handled?: boolean }>(this.op("key"), { id: this.id, ...event });
    this.lastKeyHandled = !!reply.handled;
    this.pendingClipboard = reply.clipboard ?? undefined;
    if (reply.handled) this.call("frame");
    return Uint8Array.from(reply.bytes);
  }
  private color(index: Color, background: boolean): string | undefined {
    if (index === "Default") return background ? undefined : this.theme.fg;
    if ("Spec" in index) return "#" + [index.Spec.r, index.Spec.g, index.Spec.b].map((n) => n.toString(16).padStart(2, "0")).join("");
    const value = "Named" in index ? index.Named : index.Indexed;
    if (value < 16) return this.palette[value];
    if (value >= 232) { const n = 8 + (value - 232) * 10; return "#" + n.toString(16).padStart(2, "0").repeat(3); }
    const n = value - 16, cube = (index: number) => index === 0 ? 0 : 55 + 40 * index;
    return "#" + [cube(Math.floor(n / 36)), cube(Math.floor(n / 6) % 6), cube(n % 6)].map((n) => n.toString(16).padStart(2, "0")).join("");
  }
  private packedRun = ([text,column,width,foreground,background,flags,mark]: PackedTerminalRun): CellRun => {
    const packedColor = (color: number, background: boolean) => color === 0 ? background ? undefined : this.theme.fg
      : color & 0x2000000 ? this.color({ Indexed: color & 255 }, background) : "#" + (color & 0xffffff).toString(16).padStart(6, "0");
      let fg = packedColor(foreground, false) || this.theme.fg, bg = packedColor(background, true);
      if (flags & 8) [fg, bg] = [bg || this.theme.bg, fg];
      if (mark) bg = mark === 2 ? this.theme.blue + "66" : this.theme.yellow + "4d";
      // Preserve default-color provenance for archived blocks across theme changes.
      const fgTheme = flags & 8 ? background === 0 ? "bg" : undefined : foreground === 0 ? "fg" : undefined;
      const bgTheme = flags & 8 && foreground === 0 && !mark ? "fg" : undefined;
      return { text, column, width, fg, bg, bold: !!(flags & 1), italic: !!(flags & 2), underline: !!(flags & 4), fgTheme, bgTheme };
  };
  frame(): TerminalFrame {
    if (this.paintedReply === this.reply && this.painted) return this.painted;
    const snapshot = this.reply.snapshot, packedRun = this.packedRun;
    const extraRow = this.reply.extraRow;
    let extra: TerminalFrame["extra"] = extraRow ? { above: extraRow.above, line: extraRow.runs.map(packedRun) } : undefined;
    let lines = this.reply.paintRows ? this.reply.paintRows.map(line => line.map(packedRun)) : snapshot.viewport.map((line, y) => {
      const runs: CellRun[] = []; let last: CellRun | undefined;
      for (let x = 0; x < line.length; x++) {
        const cell = line[x], flags = new Set(cell.flags.split(" | "));
        if (flags.has("WIDE_CHAR_SPACER")) continue;
        let fg = this.color(cell.fg, false) || this.theme.fg, bg = this.color(cell.bg, true);
        if (flags.has("REVERSE")) [fg, bg] = [bg || this.theme.bg, fg];
        for (const match of this.reply.searchMatches || []) {
          if ((y > match.start[0] || y === match.start[0] && x >= match.start[1]) && (y < match.end[0] || y === match.end[0] && x <= match.end[1])) bg = this.theme.yellow + "4d";
        }
        const selection = this.reply.selectionRange;
        if (selection && (y > selection.start[0] || y === selection.start[0] && x >= selection.start[1])
          && (y < selection.end[0] || y === selection.end[0] && x <= selection.end[1])
          && (!this.reply.selectionBlock || x >= selection.start[1] && x <= selection.end[1])) bg = this.theme.blue + "66";
        if (this.reply.selectionRows?.some((r) => r.row === y && x >= r.start && x <= r.end)) bg = this.theme.blue + "66";
        const bold = flags.has("BOLD"), italic = flags.has("ITALIC"), underline = flags.has("UNDERLINE");
        const fgTheme = flags.has("REVERSE") ? cell.bg === "Default" ? "bg" : undefined : cell.fg === "Default" ? "fg" : undefined;
        const bgTheme = flags.has("REVERSE") && cell.fg === "Default" && bg === this.theme.fg ? "fg" : undefined;
        const text = flags.has("HIDDEN") || cell.c === "\0" ? " " : this.reply.graphemes[`${y}:${x}`] || cell.c || " ";
        const width = flags.has("WIDE_CHAR") ? 2 : 1;
        if (last && last.fg === fg && last.bg === bg && last.bold === bold && last.italic === italic && last.underline === underline && last.fgTheme === fgTheme && last.bgTheme === bgTheme) {
          last.text += text; last.width += width;
        } else { last = { text, column: x, width, fg, bg, bold, italic, underline, fgTheme, bgTheme }; runs.push(last); }
      }
      return runs;
    });
    // Preserve glyph/style identities so the canvas display list stays cached
    // when only the viewport offset or cursor changes.
    const previous = this.painted;
    if (previous) {
      const retainLine = (line: CellRun[], old?: readonly CellRun[]) => {
        if (!old || old.length !== line.length) return line;
        let same = true;
        for (let x = 0; x < line.length; x++) {
          const a = old[x], b = line[x];
          if (a.text === b.text && a.column === b.column && a.width === b.width && a.fg === b.fg && a.bg === b.bg
            && a.bold === b.bold && a.italic === b.italic && a.underline === b.underline && a.fgTheme === b.fgTheme && a.bgTheme === b.bgTheme) line[x] = a;
          else same = false;
        }
        return same ? old as CellRun[] : line;
      };
      lines = lines.map((line, y) => retainLine(line, previous.lines[y]));
      // Fractional scroll moves the canvas without changing its ink. The edge
      // row belongs to that retained display list too, not just visible rows.
      if (extra && previous.extra && extra.above === previous.extra.above) {
        extra.line = retainLine(extra.line as CellRun[], previous.extra.line);
        if (extra.line === previous.extra.line) extra = previous.extra;
      }
      if (previous.lines.length === lines.length && lines.every((line, y) => line === previous.lines[y])) lines = previous.lines as CellRun[][];
    }
    this.paintedReply = this.reply;
    return this.painted = { lines, cursor: { x: snapshot.cursor.col, y: snapshot.cursor.row, visible: snapshot.cursor.visible, shape: snapshot.cursor.shape }, cols: snapshot.cols, rows: snapshot.rows, offset: this.reply.offset, extra };
  }
  get nextOffset() { return this.offset; }
  dispose() {
    if (this.disposed) return;
    this.disposed = true; this.pendingClipboard = undefined; this.lastKeyHandled = false;
    this.outputObserver = undefined;
    this.listeners.clear(); this.writeListeners.clear(); coreCall("dispose", { id: this.id });
  }
}
