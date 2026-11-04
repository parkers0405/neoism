import type { KeyEvent } from "@opentui/core";
import { CoreEditor, callNativeCore as coreCall, type NativeDocumentFrame } from "../../native-bridge";
import type { CrdtClientMessage, CrdtServerMessage } from "@neoism/service-client/protocol";
import type { DiagnosticItem } from "../../../neoism-protocol/typescript/types";

export interface DocumentDrain { send: CrdtClientMessage[]; seeded: boolean; bufferId: string }
export interface TerminalRun { text: string; token: string; selected: boolean; severity?: string | null; cursor?: boolean }
export interface TerminalRow { line: number; runs: TerminalRun[] }

export function nativeKey(key: Pick<KeyEvent, "name" | "sequence" | "ctrl" | "shift" | "meta" | "super">) {
  const name = ({ return: "enter", pageup: "pageup", pagedown: "pagedown" } as Record<string, string>)[key.name] || key.name;
  const printable = key.sequence && !/[\x00-\x1f\x7f]/.test(key.sequence) ? key.sequence : key.name === "space" ? " " : "";
  return { key: name, char: key.ctrl || key.meta || key.super ? "" : printable, ctrl: key.ctrl, shift: key.shift, alt: key.meta, cmd: key.super ?? false };
}

const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
export function displayWidth(source: string) { return Bun.stringWidth(source); }
export function byteToCell(source: string, byte: number) {
  let offset = 0, cell = 0;
  for (const { segment } of segmenter.segment(source)) {
    if (offset >= byte) break;
    cell += segment === "\t" ? 4 - cell % 4 : displayWidth(segment);
    offset += Buffer.byteLength(segment);
  }
  return cell;
}
export function cellToByte(source: string, target: number) {
  let offset = 0, cell = 0;
  for (const { segment } of segmenter.segment(source)) {
    const width = segment === "\t" ? 4 - cell % 4 : displayWidth(segment);
    if (cell + width > target) break;
    cell += width; offset += Buffer.byteLength(segment);
  }
  return offset;
}

/** Host the GUI's existing Rust editor through its existing Bun FFI bridge.
 * Only document spans are consumed; GPU paint coordinates never become cells. */
export class TerminalEditor {
  readonly native: CoreEditor;
  text: string;
  lines: string[];
  top = 0;
  left = 0;
  width = 80;
  height = 24;
  private revision = -1;
  private actor = Math.floor(Math.random() * 0x100000000) + 1;
  private syntax = new Map<number, NativeDocumentFrame["lines"][number]["runs"]>();
  private diagnostics: DiagnosticItem[] = [];

  constructor(readonly path: string, text: string, vim: boolean) {
    this.native = new CoreEditor(path, text, vim);
    this.text = text; this.lines = text.replace(/\r\n/g, "\n").split("\n");
    this.native.call("wrap", { enabled: false });
    this.refresh();
  }
  get frame() { return this.native.frame; }
  get id() { return this.native.id; }
  call(op: string, args: Record<string, unknown> = {}) {
    const frame = this.native.call(op, args);
    this.refresh();
    return frame;
  }
  refresh() {
    if (this.revision !== this.frame.revision) {
      this.revision = this.frame.revision;
      this.text = this.native.text(); this.lines = this.text.replace(/\r\n/g, "\n").split("\n").slice(0, this.frame.totalLines);
      this.syntax.clear();
    }
    for (const row of this.frame.lines) if (!row.continuation) this.syntax.set(row.line, row.runs);
  }
  layout(width: number, height: number) {
    width = Math.max(1, width); height = Math.max(1, height);
    if (width === this.width && height === this.height) return;
    this.width = width; this.height = height;
    // The native host uses its normal metric space for motions. Terminal paint
    // below measures Unicode graphemes independently and never rounds pixels.
    const gutter = 28 + String(this.frame.totalLines).length * 8;
    this.call("layout", { width: gutter + width * 8, height: height * 16, cellWidth: 8, rowHeight: 16 });
    this.follow();
  }
  follow() {
    const cursor = this.frame.cursor;
    if (cursor.line < this.top) this.top = cursor.line;
    if (cursor.line >= this.top + this.height) this.top = cursor.line - this.height + 1;
    const cell = byteToCell(this.lines[cursor.line] || "", cursor.col);
    if (cell < this.left) this.left = cell;
    if (cell >= this.left + this.width) this.left = cell - this.width + 1;
  }
  key(key: Parameters<typeof nativeKey>[0]) { const frame = this.call("key", nativeKey(key)); this.follow(); return frame; }
  paste(text: string) { const frame = this.call("paste", { text }); this.follow(); return frame; }
  goto(line: number, col: number) { this.call("goto", { line, col }); this.follow(); }
  scroll(rows: number) { this.top = Math.max(0, Math.min(Math.max(0, this.lines.length - this.height), this.top + rows)); this.call("scroll", { delta: rows * 16 }); }
  pointer(row: number, cell: number, extend = false) {
    const line = Math.max(0, Math.min(this.lines.length - 1, this.top + row));
    const col = cellToByte(this.lines[line] || "", this.left + Math.max(0, cell));
    if (extend) {
      // Native selection geometry is still in its own metric space.
      this.call("pointer", { x: this.frame.textX + byteToCell(this.lines[line] || "", col) * 8, y: line * 16 - this.frame.scrollY, extend: true });
    } else this.goto(line, col);
  }
  bind(): DocumentDrain { return coreCall("editor.crdt.bind", { id: this.id, path: this.path, clientId: this.actor }); }
  drain(reconnect = false): DocumentDrain { return coreCall(reconnect ? "editor.crdt.reconnect" : "editor.crdt.flush", { id: this.id }); }
  event(message: CrdtServerMessage) { this.call("crdt.event", { message }); }
  snapshot() { return this.native.snapshot(); }
  dispose() { this.native.dispose(); }
  setDiagnostics(items: DiagnosticItem[]) { this.diagnostics = items; }

  rows(focused: boolean): TerminalRow[] {
    const rows: TerminalRow[] = [];
    const selection = this.frame.selection;
    const compare = (a: number[], b: number[]) => a[0]! - b[0]! || a[1]! - b[1]!;
    const selected = (line: number, byte: number) => !!selection && compare([line, byte], selection.start) >= 0 && compare([line, byte], selection.end) < 0;
    for (let line = this.top; line < Math.min(this.lines.length, this.top + this.height); line++) {
      const runs: TerminalRun[] = [];
      const syntax = this.syntax.get(line) || [];
      let byte = 0, cell = 0, tokenCell = 0, tokenIndex = 0;
      const source = this.lines[line] || "";
      for (const { segment } of segmenter.segment(source + " ")) {
        while (tokenIndex < syntax.length && tokenCell + displayWidth(syntax[tokenIndex]!.text) <= cell) {
          tokenCell += displayWidth(syntax[tokenIndex]!.text); tokenIndex++;
        }
        const token = syntax[tokenIndex];
        const width = segment === "\t" ? 4 - cell % 4 : displayWidth(segment);
        if (cell + width > this.left && cell < this.left + this.width) {
          const cursor = focused && line === this.frame.cursor.line && byte === this.frame.cursor.col;
          const text = segment === "\t" ? " ".repeat(width) : /[\x00-\x1f\x7f]/.test(segment) ? " " : segment;
          const clipped = cell < this.left || cell + width > this.left + this.width ? " ".repeat(Math.max(0, Math.min(cell + width, this.left + this.width) - Math.max(cell, this.left))) : text;
          const diagnostic = this.diagnostics.find(item => line >= item.line && line <= item.end_line && (line !== item.line || byte >= item.col) && (line !== item.end_line || byte < Math.max(item.col + 1, item.end_col)));
          const next: TerminalRun = { text: clipped, token: token?.token || "Normal", selected: selected(line, byte), severity: diagnostic?.severity || token?.severity, cursor };
          const last = runs.at(-1);
          if (last && last.token === next.token && last.selected === next.selected && last.severity === next.severity && last.cursor === next.cursor) last.text += clipped;
          else runs.push(next);
        }
        byte += Buffer.byteLength(segment); cell += width;
        if (cell >= this.left + this.width) break;
      }
      rows.push({ line, runs });
    }
    return rows;
  }
}