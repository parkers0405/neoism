import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function harness() {
  const panel: any = Object.create(TerminalPanel.prototype);
  const calls: unknown[] = [];
  panel.agentScrollbarPointerId = null;
  panel.agentSelectionActive = false;
  panel.canvas = {
    setPointerCapture: (id: number) => calls.push(["capture", id]),
    releasePointerCapture: (id: number) => calls.push(["release", id]),
  };
  panel.canvasLogicalPoint = () => ({ x: 90, y: 120 });
  panel.scheduleDraw = () => {};
  panel.wasmAdapter = {
    agentPointerDown: () => ({ handled: true, selecting: false, copy: null, link: null }),
    // True denotes an active drag, including a clamped rail at either bound.
    agentDragScrollbar: (x: number, y: number) => { calls.push(["drag", x, y]); return true; },
    agentEndScrollbarDrag: () => { calls.push(["end"]); return true; },
  };
  const event = (pointerId = 7, button = 0) => ({
    pointerId, button, pointerType: "mouse", preventDefault: () => {},
  });
  return { panel, calls, event };
}

test("rail press captures its pointer and owns moves ahead of selection and overlays", () => {
  const { panel, calls, event } = harness();
  assert.equal(panel.agentPointerDownAt(90, 100, event()), true);
  assert.equal(panel.agentScrollbarPointerId, 7);
  assert.deepEqual(calls, [["drag", 90, 100], ["capture", 7]]);
  panel.agentSelectionActive = true;
  // Other routing dependencies are deliberately absent: a rail move must exit first.
  panel.handlePointerMove(event());
  assert.deepEqual(calls.at(-1), ["drag", 90, 120]);
  const count = calls.length;
  panel.handlePointerMove(event(8));
  panel.handlePointerDown(event(8));
  panel.handlePointerUp(event(8));
  panel.handlePointerCancel(event(8));
  assert.equal(calls.length, count);
  assert.equal(panel.agentScrollbarPointerId, 7);
});

for (const method of ["handlePointerUp", "handlePointerCancel"] as const) {
  test(`${method} ends both-axis rail ownership and releases capture`, () => {
    const { panel, calls, event } = harness();
    panel.agentPointerDownAt(90, 100, event());
    panel[method](event());
    assert.equal(panel.agentScrollbarPointerId, null);
    assert.deepEqual(calls.slice(-2), [["end"], ["release", 7]]);
    assert.equal(panel.endAgentScrollbarDrag(7), false);
  });
}

test("secondary button release does not end primary rail drag", () => {
  const { panel, event } = harness();
  panel.agentPointerDownAt(90, 100, event());
  panel.handlePointerUp(event(7, 2));
  assert.equal(panel.agentScrollbarPointerId, 7);
});

test("touch tap ends the shared rail drag without capturing a PointerEvent", () => {
  const { panel, calls } = harness();
  panel.agentPointerDownAt(90, 100);
  assert.equal(panel.agentScrollbarPointerId, null);
  assert.deepEqual(calls, [["drag", 90, 100], ["end"]]);
});

test("capture failure still retains logical ownership and cancellation cleanup", () => {
  const { panel, calls, event } = harness();
  panel.canvas.setPointerCapture = () => { throw new Error("detached"); };
  panel.canvas.releasePointerCapture = () => { throw new Error("already lost"); };
  panel.agentPointerDownAt(90, 100, event());
  panel.handlePointerCancel(event());
  assert.equal(panel.agentScrollbarPointerId, null);
  assert.deepEqual(calls.at(-1), ["end"]);
});

test("non-rail presses retain timeline selection behavior without capture", () => {
  const { panel, calls, event } = harness();
  panel.wasmAdapter.agentPointerDown = () => ({ handled: true, selecting: true });
  panel.wasmAdapter.agentDragScrollbar = () => false;
  panel.agentPointerDownAt(30, 50, event());
  assert.equal(panel.agentSelectionActive, true);
  assert.equal(panel.agentScrollbarPointerId, null);
  assert.deepEqual(calls, []);
});
