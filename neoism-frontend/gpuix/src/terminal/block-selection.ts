import type { CellRun } from "./buffer";

export interface BlockTextRow { id: string; top: number; text?: string; runs?: readonly CellRun[] }
export interface BlockTextPoint { id: string; column: number }
export interface BlockTextSelection { anchor: BlockTextPoint; head: BlockTextPoint }
const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

// Keep graphemes whole, and use native run columns rather than string offsets.
function cells(row: BlockTextRow) {
  const result: Array<{ text: string; start: number; end: number }> = [];
  let column = 0;
  for (const run of row.runs || [{ text: row.text || "", column: 0 }]) {
    while (column < run.column) result.push({ text: " ", start: column, end: ++column });
    column = run.column;
    for (const { segment } of graphemes.segment(run.text)) {
      const end = column + Bun.stringWidth(segment);
      result.push({ text: segment, start: column, end }); column = end;
    }
  }
  return result;
}

export function blockTextPoint(rows: readonly BlockTextRow[], y: number, column: number): BlockTextPoint | undefined {
  if (!rows.length) return;
  const row = rows.find((row, i) => i === rows.length - 1 || y < rows[i + 1].top) || rows[0];
  const units = cells(row), width = units.at(-1)?.end || 0;
  column = Math.max(0, Math.min(width, Math.round(column)));
  const unit = units.find(unit => column > unit.start && column < unit.end);
  if (unit) column = column - unit.start < unit.end - column ? unit.start : unit.end;
  return { id: row.id, column };
}

export function blockTextRanges(rows: readonly BlockTextRow[], selection?: BlockTextSelection) {
  if (!selection) return [];
  let start = rows.findIndex(row => row.id === selection.anchor.id), end = rows.findIndex(row => row.id === selection.head.id);
  if (start < 0 || end < 0) return [];
  let left = selection.anchor.column, right = selection.head.column;
  if (start > end || start === end && left > right) { [start, end] = [end, start]; [left, right] = [right, left]; }
  if (start === end && left === right) return [];
  return rows.slice(start, end + 1).map((row, index) => {
    const units = cells(row), width = units.at(-1)?.end || 0;
    const from = index === 0 ? left : 0, to = index === end - start ? right : width;
    const newline = index < end - start, text = units.filter(unit => unit.end > from && unit.start < to).map(unit => unit.text).join("");
    return { row, from, to, newline, text: newline ? text.trimEnd() : text };
  });
}

export function blockSelectedText(rows: readonly BlockTextRow[], selection?: BlockTextSelection) {
  return blockTextRanges(rows, selection).map(range => range.text).join("\n");
}