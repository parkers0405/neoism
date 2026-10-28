import { toArrayBuffer, type Pointer } from "bun:ffi";

/** Display-list commands for the native `<canvas>` element (see the GPUIX
 * patch, custom_elements/canvas.rs). One canvas paints hundreds of primitives
 * as a single GPUI element: no per-primitive element, layout node or paint
 * pass. Coordinates are relative to the canvas box; colors are 0xRRGGBBAA. */
export type CanvasOp =
  | { k: "r"; b: number[]; c: number; r?: number[]; bc?: number; bw?: number }
  | { k: "i"; id: number; b: number[]; r?: number[]; s?: number[] }
  | { k: "t"; x: number; y: number; t: string; s: number; c: number; f: string; w?: number; i?: boolean; lh?: number; aw?: number; al?: "l" | "c" | "r" }
  | { k: "p"; c: number; p: number[][] }
  | { k: "l"; c: number; w: number; p: number[] }
  | { k: "cl"; b: number[] }
  | { k: "u" };

const clamp = (value: number) => Math.max(0, Math.min(255, Math.round(value)));
const pack = (r: number, g: number, b: number, a: number) => ((clamp(r) << 24) | (clamp(g) << 16) | (clamp(b) << 8) | clamp(a)) >>> 0;
const parsed = new Map<string, number>();
/** `#rgb`, `#rrggbb`, `#rrggbbaa` or `rgba(r,g,b,a)`; `alpha` multiplies. */
export function cssColor(color: string, alpha = 1): number {
  let value = parsed.get(color);
  if (value === undefined) {
    const hex = color.trim().replace(/^#/, "");
    if (/^[0-9a-f]{3}$/i.test(hex)) value = pack(...[...hex].map(d => parseInt(d + d, 16)) as [number, number, number], 255);
    else if (/^[0-9a-f]{6}([0-9a-f]{2})?$/i.test(hex)) value = pack(parseInt(hex.slice(0, 2), 16), parseInt(hex.slice(2, 4), 16), parseInt(hex.slice(4, 6), 16), hex.length === 8 ? parseInt(hex.slice(6, 8), 16) : 255);
    else {
      const parts = color.match(/rgba?\(([^)]*)\)/i)?.[1].split(",").map(Number);
      value = parts && parts.length >= 3 ? pack(parts[0], parts[1], parts[2], (parts[3] ?? 1) * 255) : 0xe8e8e8ff;
    }
    if (parsed.size > 4096) parsed.clear();
    parsed.set(color, value);
  }
  return alpha === 1 ? value : ((value & 0xffffff00) | clamp((value & 0xff) * alpha)) >>> 0;
}
/** Painter colors: unit floats, or bytes when `bytes`. */
export function paintColor(color: readonly number[], bytes = false): number {
  const scale = bytes ? 1 : 255;
  return pack(color[0] * scale, color[1] * scale, color[2] * scale, (color[3] ?? (bytes ? 255 : 1)) * scale);
}

export interface CanvasRaster { id: number; ptr: number; len: number; width: number; height: number }
interface ImageRenderer {
  putPaintImage?: (key: number, width: number, height: number, pixels: Buffer, format?: string) => void;
  dropPaintImage?: (key: number) => void;
}
/** Raster id counters are per native source; keys must not collide. */
export const RasterSource = { wordmark: 0, editor: 1, painter: 2, tree: 3, paintImage: 4 } as const;
export const rasterKey = (source: number, id: number) => id * 8 + source;
const refcounts = new WeakMap<object, Map<number, number>>();
function counts(renderer: object) {
  let map = refcounts.get(renderer); if (!map) { map = new Map(); refcounts.set(renderer, map); }
  return map;
}

/** Keep a bounded set of recently visible textures resident when scrolling
 * back. These are extra leases; eviction cannot release another canvas's ink. */
export class RecentCanvasImages {
  private retained = new Map<number, number>();
  private bytes = 0;
  constructor(private readonly renderer: object, private readonly budget: number) {}
  use(source: number, raster: CanvasRaster) {
    const key = rasterKey(source, raster.id), previous = this.retained.get(key);
    if (previous !== undefined) {
      this.retained.delete(key); this.retained.set(key, previous); return;
    }
    if (raster.len > this.budget || this.budget <= 0) return;
    const map = counts(this.renderer), count = map.get(key) || 0;
    if (!count) {
      const pixels = Buffer.from(toArrayBuffer(raster.ptr as unknown as Pointer, 0, raster.len));
      (this.renderer as ImageRenderer).putPaintImage?.(key, raster.width, raster.height, pixels, "rgba");
    }
    map.set(key, count + 1);
    this.retained.set(key, raster.len); this.bytes += raster.len;
    while (this.bytes > this.budget) this.release(this.retained.keys().next().value!);
  }
  private release(key: number) {
    this.bytes -= this.retained.get(key)!; this.retained.delete(key);
    const map = counts(this.renderer), count = (map.get(key) || 0) - 1;
    if (count > 0) map.set(key, count);
    else { map.delete(key); (this.renderer as ImageRenderer).dropPaintImage?.(key); }
  }
  dispose() { for (const key of this.retained.keys()) this.release(key); }
}

/** Frame-scoped image references for one canvas. Pixels upload when a key is
 * first used (copied synchronously out of the native raster cache, which only
 * guarantees the pointer for the frame that produced it). A key unused by a
 * frame is released one frame later, so a draw that still shows the previous
 * display list never loses its image. */
export class CanvasImages {
  private current = new Set<number>();
  private previous = new Set<number>();
  private deferred: number[] = [];
  constructor(private readonly renderer: object,private readonly retain?:(source:number,raster:CanvasRaster)=>void) {}
  use(source: number, raster: CanvasRaster): number {
    const key = rasterKey(source, raster.id);
    if (this.current.has(key)) return key;
    // The session lease must exist before this frame's own reference. A
    // generation may release its canvas before the atomic replacement batch.
    this.retain?.(source,raster);
    this.current.add(key);
    if (!this.previous.has(key)) {
      const map = counts(this.renderer), count = map.get(key) || 0;
      if (!count) {
        const pixels = Buffer.from(toArrayBuffer(raster.ptr as unknown as Pointer, 0, raster.len));
        (this.renderer as ImageRenderer).putPaintImage?.(key, raster.width, raster.height, pixels, "rgba");
      }
      map.set(key, count + 1);
    }
    return key;
  }
  /** Close the frame started since the last commit. */
  commit() {
    for (const key of this.deferred) this.release(key);
    this.deferred = [...this.previous].filter(key => !this.current.has(key));
    this.previous = this.current; this.current = new Set();
  }
  dispose() {
    for (const key of this.deferred) this.release(key);
    for (const key of new Set([...this.previous, ...this.current])) this.release(key);
    this.deferred = []; this.previous.clear(); this.current.clear();
  }
  private release(key: number) {
    const map = counts(this.renderer), count = (map.get(key) || 0) - 1;
    if (count > 0) { map.set(key, count); return; }
    map.delete(key); (this.renderer as ImageRenderer).dropPaintImage?.(key);
  }
}
