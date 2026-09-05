import assert from "node:assert/strict";
import test from "node:test";

import {
  AgentTouchScrollOwnership,
  agentMomentumAxis,
} from "./agentTouchScrollOwnership.ts";

test("vertical drag starting on a diff locks to the outer timeline across chunks", () => {
  const gesture = new AgentTouchScrollOwnership();
  const calls: Array<[number, number, number, number]> = [];
  gesture.begin(40, 180);
  const routes = {
    dragVertical: (x: number, y: number, dy: number, owner: 0 | 1 | 2) => {
      calls.push([x, y, dy, owner]);
      return 2;
    },
    dragHorizontal: () => false,
  };

  assert.equal(gesture.route(42, 160, 2, -20, true, routes), true);
  assert.equal(gesture.route(44, 80, 2, -80, true, routes), true);
  assert.deepEqual(calls, [[40, 180, -20, 0], [40, 180, -80, 2]]);
  assert.equal(gesture.currentOwner(), "timeline");
});

test("release momentum retains the resolved owner and touch-down anchor", () => {
  const gesture = new AgentTouchScrollOwnership();
  const calls: Array<[number, number, number]> = [];
  gesture.begin(25, 90);
  const routes = {
    dragVertical: (x: number, y: number, _dy: number, owner: 0 | 1 | 2) => {
      calls.push([x, y, owner]);
      return owner || 1;
    },
    dragHorizontal: () => false,
  };

  assert.equal(gesture.route(25, 110, 0, 20, true, routes), true);
  // A later coordinate is the momentum frame's stale/current anchor, but the
  // picker/side panel still receives it through the original hit point.
  assert.equal(gesture.route(25, 260, 0, 14, true, routes), true);
  assert.deepEqual(calls, [[25, 90, 0], [25, 90, 1]]);
  assert.equal(gesture.currentOwner(), "nested");
});

test("a zero-consumption timeline bound reports false", () => {
  const gesture = new AgentTouchScrollOwnership();
  gesture.begin(10, 100);
  assert.equal(gesture.route(10, 80, 0, -20, true, {
    dragVertical: () => 0,
    dragHorizontal: () => false,
  }), false);
  assert.equal(gesture.currentOwner(), null);
});

test("horizontal nested content remains axis-locked", () => {
  const gesture = new AgentTouchScrollOwnership();
  const deltas: number[] = [];
  gesture.begin(100, 100);
  const routes = {
    dragVertical: () => 2,
    dragHorizontal: (_x: number, _y: number, dx: number) => (deltas.push(dx), true),
  };
  assert.equal(gesture.route(80, 98, -20, -2, true, routes), true);
  assert.equal(gesture.route(70, 130, -10, 32, true, routes), true);
  assert.deepEqual(deltas, [20, 10]);
  assert.equal(gesture.currentOwner(), "horizontal");
});

test("reset and surface disappearance clear ownership", () => {
  const gesture = new AgentTouchScrollOwnership();
  gesture.begin(10, 100);
  const routes = { dragVertical: () => 2, dragHorizontal: () => false };
  assert.equal(gesture.route(10, 90, 0, -10, true, routes), true);
  assert.equal(gesture.route(10, 80, 0, -10, false, routes), false);
  assert.equal(gesture.currentOwner(), null);
  gesture.begin(10, 100);
  gesture.reset();
  assert.equal(gesture.route(10, 90, 0, -10, true, routes), false);
});

test("zero touchend sample preserves locked ownership and release axis", () => {
  const gesture = new AgentTouchScrollOwnership();
  let verticalCalls = 0;
  gesture.begin(10, 100);
  const routes = {
    dragVertical: () => (verticalCalls += 1, 2),
    dragHorizontal: () => false,
  };

  assert.equal(gesture.route(10, 80, 0, -20, true, routes), true);
  assert.equal(gesture.route(10, 80, 0, 0, true, routes), true);
  assert.equal(verticalCalls, 1);
  assert.equal(gesture.currentOwner(), "timeline");
  assert.equal(agentMomentumAxis(gesture.currentOwner()), "y");
  assert.equal(agentMomentumAxis("nested"), "y");
  assert.equal(agentMomentumAxis("horizontal"), "x");
  assert.equal(agentMomentumAxis(null), "dominant");
});