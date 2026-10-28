import { expect, test } from "bun:test";
import { treeReveal, visibleTreeRange } from "../src/ui/tree-viewport";
import { easeOutCubic } from "../src/design/animation";

test("large file trees paint only viewport rows, including spring displacement", () => {
  expect(visibleTreeRange(50000, 400, 20, 25000, 0)).toEqual([24998, 25022]);
  expect(visibleTreeRange(50000, 400, 20, 25000, -30)).toEqual([24999, 25024]);
  expect(visibleTreeRange(3, 400, 20, 0, 10)).toEqual([0, 3]);
});

test("virtual rows retain the original stagger and exact remaining cubic reveal", () => {
  expect(treeReveal(1000, 100, 320, 12)).toEqual({ opacity: 1, delay: 0, duration: 0 });
  const phase = treeReveal(200, 5, 320, 12), remaining = 50;
  const continued = phase.opacity + (1 - phase.opacity) * easeOutCubic(remaining / (phase.duration * 1000));
  expect(continued).toBeCloseTo(easeOutCubic((200 + remaining - 5 * 12) / 320), 10);
  expect(treeReveal(20, 5, 320, 12)).toEqual({ opacity: 0, delay: 0.04, duration: 0.32 });
});
