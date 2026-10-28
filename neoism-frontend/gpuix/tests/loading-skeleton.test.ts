import { expect, test } from "bun:test";
import { loadingSkeletonBars } from "../src/ui/LoadingSkeleton";

test("original tree loader keeps its depth pattern, clipping and travelling shimmer", () => {
  const hidden = loadingSkeletonBars("tree", 360, 600, 1, 0);
  expect(hidden).toHaveLength(24);
  expect(hidden.every(bar => bar.alpha === 0)).toBe(true);
  const bars = loadingSkeletonBars("tree", 360, 600, 1, 0.325);
  expect(bars[0].x).toBe(28.25);
  expect(bars[4].x).toBe(42.25);
  expect(bars[8].x).toBe(56.25);
  expect(bars[0].alpha).toBeCloseTo(0.24);
  expect(bars[2].alpha).toBeLessThan(bars[0].alpha);
  expect(loadingSkeletonBars("tree", 360, 54, 1, 1)).toHaveLength(2);
});

test("picker loader uses all three original two-line rows and exact fade/period", () => {
  const bars = loadingSkeletonBars("picker", 400, 138, 1, 0.09);
  expect(bars).toHaveLength(6);
  expect(bars[0]).toMatchObject({ x: 22, y: 8, width: 356 * 0.42, height: 12, radius: 6 });
  expect(bars[1].alpha).toBeCloseTo(bars[0].alpha * 0.7);
  const a = loadingSkeletonBars("picker", 400, 138, 1, 0.5);
  const b = loadingSkeletonBars("picker", 400, 138, 1, 1.8);
  expect(a.map(bar => bar.alpha)).toEqual(b.map(bar => expect.closeTo(bar.alpha)));
  expect(loadingSkeletonBars("picker", 400, 92, 1, 1)).toHaveLength(4);
});

test("document loader honors original 80ms grace and 240ms cubic reveal", () => {
  expect(loadingSkeletonBars("document", 1000, 300, 1, 0.08).every(bar => bar.alpha === 0)).toBe(true);
  expect(loadingSkeletonBars("document", 1000, 300, 1, 0.32)[0].alpha).toBeCloseTo(0.14);
  expect(loadingSkeletonBars("document", 1000, 300, 1, 0.2)[0].alpha).toBeCloseTo(0.14 * 0.875);
});

test("native loading card reserves three real rows before its catalog arrives", async () => {
  const { coreCall } = await import("../src/native-core");
  const geometry = { anchor: [100, 700, 600, 120], scale: 1 };
  const { id } = coreCall<{ id: number }>("picker.new", { kind: "model", title: "Models", options: [], loading: true });
  try {
    const loading = coreCall<{ rect: number[]; loading: boolean }>("picker.frame", { id, ...geometry });
    const settled = coreCall<{ rect: number[]; loading: boolean }>("picker.options", { id, ...geometry, options: [{ title: "Model", value: "provider/model" }], loading: false });
    expect(loading.loading).toBe(true);
    expect(loading.rect[3] - settled.rect[3]).toBe(2 * 46);
    expect(settled.loading).toBe(false);
  } finally { coreCall("picker.dispose", { id }); }
});
