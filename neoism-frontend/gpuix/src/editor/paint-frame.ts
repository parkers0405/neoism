import type { EditorFrame, EditorRun } from "../native-core";

export function unpackEditorPaint(frame: EditorFrame): EditorFrame {
  if (frame.packedRuns) {
    for (const line of frame.lines) {
      line.runs = (line.runs as unknown as Array<[string, number, number, string, boolean, string | null]>).map(
        ([text,x,width,token,selected,severity]) => ({ text,x,width,token,selected,severity }));
    }
    frame.packedRuns = false;
  }
  return frame;
}

function sameRecord<T extends object>(a: T, b: T): boolean {
  const keys = Object.keys(a) as Array<keyof T>;
  return keys.length === Object.keys(b).length && keys.every(key => a[key] === b[key]);
}
function share<T>(before: T[], next: T[], equal: (a: T, b: T) => boolean): T[] {
  let same = before.length === next.length;
  for (let i = 0; i < next.length; i++) {
    if (before[i] !== undefined && equal(before[i], next[i])) next[i] = before[i];
    else same = false;
  }
  return same ? before : next;
}
function shareJson(before: unknown, next: unknown): unknown {
  if (before === next || !before || !next || typeof before !== "object" || typeof next !== "object") return next;
  if (Array.isArray(before) !== Array.isArray(next)) return next;
  const a = before as Record<string, unknown>, b = next as Record<string, unknown>;
  const keys = Object.keys(b); let same = keys.length === Object.keys(a).length;
  for (const key of keys) { b[key] = shareJson(a[key], b[key]); if (b[key] !== a[key]) same = false; }
  return same ? before : next;
}

/** Keep text host nodes untouched during caret and sub-row scroll frames. */
export function shareEditorPaint(before: EditorFrame, next: EditorFrame): EditorFrame {
  const previousRows = new Map(before.lines.map(line => [visualRow(before,line),line]));
  next.lines = next.lines.map(line => {
    const old=previousRows.get(visualRow(next,line));
    if (!old || old.line!==line.line || old.continuation!==line.continuation) return line;
    line.runs=share<EditorRun>(old.runs,line.runs,sameRecord);
    if(old.raster&&line.raster&&sameRecord(old.raster,line.raster))line.raster=old.raster;
    return old.y===line.y && old.runs===line.runs && old.raster===line.raster ? old : line;
  });
  if (before.lines.length===next.lines.length && next.lines.every((line,i)=>line===before.lines[i])) next.lines=before.lines;
  next.extraCarets = share(before.extraCarets, next.extraCarets, sameRecord);
  next.searchHits = share(before.searchHits, next.searchHits, sameRecord);
  next.cursorQuad = share(before.cursorQuad, next.cursorQuad, (a, b) => a.length === b.length && a.every((n, i) => n === b[i]));
  next.lsp = shareJson(before.lsp, next.lsp) as EditorFrame["lsp"];
  return next;
}

function visualRow(frame: EditorFrame, line: EditorFrame["lines"][number]) {
  return Math.round((line.y+frame.scrollY)/Math.max(1,frame.rowHeight));
}
export type EditorPaintRow = EditorFrame["lines"][number] & { visual: number };
/** Source glyphs stay at document coordinates. One parent offset scrolls the
 * grid, and keyed rows keep existing glyph hosts when an edge row enters. */
export function documentPaintRows(previous: EditorPaintRow[], frame: EditorFrame): EditorPaintRow[] {
  const oldRows=new Map(previous.map(row=>[row.visual,row]));
  const rows=frame.lines.map(line=>{
    const visual=visualRow(frame,line), y=visual*frame.rowHeight, old=oldRows.get(visual);
    return old && old.line===line.line && old.continuation===line.continuation && old.y===y && old.runs===line.runs && old.raster===line.raster
      ? old : {...line,y,visual};
  });
  return previous.length===rows.length && rows.every((row,i)=>row===previous[i]) ? previous : rows;
}

/** SVG elements rasterize at their host size. Bound the exact same trail quad
 * to its pixels instead of allocating a viewport-sized image every frame. */
export function cursorPaint(points: readonly number[][]) {
  if (!points.length) return { left: 0, top: 0, width: 1, height: 1, points: "" };
  const left = Math.floor(Math.min(...points.map(p => p[0]))) - 1;
  const top = Math.floor(Math.min(...points.map(p => p[1]))) - 1;
  const width = Math.max(1, Math.ceil(Math.max(...points.map(p => p[0]))) - left + 1);
  const height = Math.max(1, Math.ceil(Math.max(...points.map(p => p[1]))) - top + 1);
  return { left, top, width, height, points: points.map(p => `${p[0]-left},${p[1]-top}`).join(" ") };
}

export function sameBounds(a: { x: number; y: number; width: number; height: number }, b: typeof a) {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}
