import { expect, test } from "bun:test";
import { animateRailIcon, railIconMotion } from "../src/ui/activity-rail-motion";
import type { PaintCommand } from "../src/ui/PaintLayer";

test("rail hover enlarges and lifts icons, and click motion settles exactly", () => {
  expect(railIconMotion(0, Infinity, 1)).toEqual({ zoom: 1, lift: -0 });
  expect(railIconMotion(1, Infinity, 2)).toEqual({ zoom: 1.12, lift: -4 });
  expect(railIconMotion(1, 70, 1).zoom).toBeLessThan(1.12);
  expect(railIconMotion(1, 210, 1).zoom).toBeGreaterThan(1.12);
  expect(railIconMotion(1, 420, 1)).toEqual(railIconMotion(1, Infinity, 1));
});

test("rail animation transforms cached glyph ink without moving background or hit geometry", () => {
  const rect = [6, 6, 40, 40];
  const glyph: PaintCommand = { kind: "raster-text", rect: [16, 16, 20, 20], raster: { id: 1, ptr: 1, len: 1600, width: 20, height: 20, dx: 0, dy: 0 }, layer: 0 };
  const background: PaintCommand = { kind: "rect", rect: [0, 0, 52, 800], color: [0, 0, 0, 1], radii: [0, 0, 0, 0], depth: 0, order: 0, layer: 0 };
  expect(animateRailIcon(glyph, rect, 1.2, -2)).toMatchObject({ rect: [14, 12, 24, 24], raster: glyph.raster });
  expect(animateRailIcon(background, rect, 1.2, -2)).toBe(background);
  expect(rect).toEqual([6, 6, 40, 40]);
});