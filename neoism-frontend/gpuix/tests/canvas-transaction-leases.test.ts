import { expect, test } from "bun:test";
import { trackCanvasTransactions, retireCanvasImage } from "../src/ui/canvas-transactions";

test("non-canvas batches still release images on destruction and preserve live leases", () => {
  const drops: number[] = [];
  let destroyed: number[] = [];
  const renderer = { applyBatch: (_json: string) => destroyed, dropPaintImage: (key: number) => { drops.push(key); } };
  trackCanvasTransactions(renderer);
  renderer.applyBatch(JSON.stringify([["setCustomProp", 7, "commands", '[{"k":"i","id":42}]']]));
  retireCanvasImage(renderer, 42);
  renderer.applyBatch(JSON.stringify([["setStyle", 7, { opacity: 0.5 }]]));
  expect(drops).toEqual([]);
  destroyed = [7];
  renderer.applyBatch(JSON.stringify([["destroy", 7]]));
  expect(drops).toEqual([42]);
});
