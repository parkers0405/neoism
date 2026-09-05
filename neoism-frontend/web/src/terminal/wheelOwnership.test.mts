import assert from "node:assert/strict";
import test from "node:test";
import { routeWheelToFirstOwner } from "./wheelOwnership.ts";

type Rect = { x: number; y: number; w: number; h: number };
const contains = (r: Rect, x: number, y: number) =>
  x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;

function routeAt(pointer: { x: number; y: number }, panel: Rect, atBoundary = false) {
  const counters = { panel: 0, page: 0, terminal: 0 };
  const owner = routeWheelToFirstOwner([
    {
      owner: "chrome",
      route: () => {
        if (!contains(panel, pointer.x, pointer.y)) return false;
        if (!atBoundary) counters.panel += 1;
        // Ownership is geometric and remains true at the scroll boundary.
        return true;
      },
    },
    { owner: "active-content", route: () => (++counters.page, true) },
    { owner: "terminal", route: () => (++counters.terminal, true) },
  ]);
  return { owner, counters };
}

for (const [name, rect] of [
  ["tree", { x: 0, y: 80, w: 240, h: 600 }],
  ["notes", { x: 760, y: 80, w: 240, h: 600 }],
] as const) {
  test(`wheel over ${name} moves only that panel`, () => {
    const result = routeAt({ x: rect.x + 20, y: rect.y + 20 }, rect);
    assert.equal(result.owner, "chrome");
    assert.deepEqual(result.counters, { panel: 1, page: 0, terminal: 0 });
  });

  test(`wheel at ${name} boundary does not chain into the page`, () => {
    const result = routeAt({ x: rect.x + 20, y: rect.y + 20 }, rect, true);
    assert.equal(result.owner, "chrome");
    assert.deepEqual(result.counters, { panel: 0, page: 0, terminal: 0 });
  });
}