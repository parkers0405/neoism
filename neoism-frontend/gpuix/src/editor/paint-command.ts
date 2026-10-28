import type { PaintCommand, PaintRaster } from "../ui/PaintLayer";

function numbersEqual(a: readonly number[] | null | undefined, b: typeof a): boolean {
  if (a === b) return true;
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function rasterEqual(a: PaintRaster | null | undefined, b: typeof a): boolean {
  return a === b || !!a && !!b && a.id === b.id && a.ptr === b.ptr && a.len === b.len
    && a.width === b.width && a.height === b.height && a.scale === b.scale
    && a.dx === b.dx && a.dy === b.dy;
}

function commandEqual(a: PaintCommand, b: PaintCommand): boolean {
  if (a === b) return true;
  if (a.kind !== b.kind || a.layer !== b.layer) return false;
  if (a.kind === "raster-text" && b.kind === "raster-text") return numbersEqual(a.rect, b.rect) && numbersEqual(a.clip, b.clip) && rasterEqual(a.raster, b.raster);
  if (a.kind === "raster-text" || b.kind === "raster-text") return false;
  if (a.kind === "image" && b.kind === "image") return a.depth === b.depth && numbersEqual(a.rect, b.rect) && numbersEqual(a.source, b.source) && rasterEqual(a.raster, b.raster);
  if (a.kind === "image" || b.kind === "image" || !numbersEqual(a.color, b.color)) return false;
  if (a.kind === "rect" && b.kind === "rect") {
    return a.depth === b.depth && a.order === b.order && numbersEqual(a.rect, b.rect)
      && numbersEqual(a.radii, b.radii) && numbersEqual(a.clip, b.clip);
  }
  if(a.kind==="ring"&&b.kind==="ring"){
    return a.depth===b.depth&&a.order===b.order&&a.outer.length===b.outer.length&&a.inner.length===b.inner.length
      &&a.outer.every((point,index)=>numbersEqual(point,b.outer[index]))&&a.inner.every((point,index)=>numbersEqual(point,b.inner[index]));
  }
  if (a.kind === "polygon" && b.kind === "polygon") {
    if (a.depth !== b.depth || a.points.length !== b.points.length) return false;
    for (let i = 0; i < a.points.length; i++) if (!numbersEqual(a.points[i], b.points[i])) return false;
    return true;
  }
  if (a.kind === "text" && b.kind === "text") {
    return a.x === b.x && a.y === b.y && a.text === b.text && a.size === b.size && a.width === b.width
      && a.bold === b.bold && a.italic === b.italic && a.extrude === b.extrude && a.family === b.family
      && numbersEqual(a.clip, b.clip) && rasterEqual(a.raster, b.raster);
  }
  return false;
}

/** Keep unchanged native paint nodes intact during caret and hover frames.
 * Compare every serialized paint property; raster identity alone cannot prove
 * that its placement, clipping or logical pixel scale stayed unchanged. */
export function sharePaintCommands(before: PaintCommand[], next: PaintCommand[]): PaintCommand[] {
  let unchanged = before.length === next.length;
  for (let i = 0; i < next.length; i++) {
    if (before[i] && commandEqual(before[i], next[i])) next[i] = before[i];
    else unchanged = false;
  }
  return unchanged ? before : next;
}
