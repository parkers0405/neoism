import { expect, test } from "bun:test";
import { ptr } from "bun:ffi";
import { CanvasImages, RecentCanvasImages, rasterKey } from "../src/ui/canvas";

function fixture(budget = 8) {
  const pixels = Buffer.alloc(4, 255), uploads: number[] = [], drops: number[] = [];
  const renderer = { putPaintImage: (key: number) => { uploads.push(key); }, dropPaintImage: (key: number) => { drops.push(key); } };
  const recent = new RecentCanvasImages(renderer, budget);
  const canvas = new CanvasImages(renderer, (source, raster) => recent.use(source, raster));
  const raster = (id: number) => ({ id, ptr: Number(ptr(pixels)), len: pixels.length, width: 1, height: 1 });
  return { pixels, uploads, drops, renderer, recent, canvas, raster };
}

test("returning to offscreen text reuses its resident texture", () => {
  const f = fixture();
  f.canvas.use(2, f.raster(1)); f.canvas.commit();
  f.canvas.commit(); f.canvas.commit();
  expect(f.drops).toEqual([]);
  f.canvas.use(2, f.raster(1)); f.canvas.commit();
  expect(f.uploads).toEqual([rasterKey(2, 1)]);
  f.canvas.dispose(); f.recent.dispose(); f.recent.dispose();
  expect(f.drops).toEqual(f.uploads);
});

test("budget evicts least recently used textures without releasing another canvas", () => {
  const f = fixture(), other = new CanvasImages(f.renderer);
  other.use(2, f.raster(2)); other.commit();
  f.recent.use(2, f.raster(1)); f.recent.use(2, f.raster(2));
  f.recent.use(2, f.raster(1)); f.recent.use(2, f.raster(3));
  expect(f.drops).toEqual([]); // 2 is evicted but still visible elsewhere.
  other.dispose();
  expect(f.drops).toEqual([rasterKey(2, 2)]);
  f.recent.dispose();
  expect([...f.drops].sort()).toEqual([...f.uploads].sort());
});

test("textures larger than the retention budget keep only frame leases", () => {
  const f = fixture(3);
  f.canvas.use(2, f.raster(1)); f.canvas.commit();
  f.canvas.commit(); f.canvas.commit();
  expect(f.drops).toEqual(f.uploads);
  f.canvas.dispose(); f.recent.dispose();
  expect(f.drops).toHaveLength(1);
});
