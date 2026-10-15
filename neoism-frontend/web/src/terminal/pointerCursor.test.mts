import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function fixture(overlay: boolean) {
  const panel = Object.create(TerminalPanel.prototype) as any;
  const seen: unknown[] = [];
  Object.assign(panel, {
    canvas: { style: { cursor: "" }, getBoundingClientRect: () => ({ left: 0, top: 0 }) },
    markdownLayer: { style: { cursor: "" } },
    tabGestureOwnership: { owns: () => false },
    canvasLogicalPoint: () => ({ x: 42, y: 17 }),
    wasmAdapter: {
      pointerOverlayActive: () => overlay,
      pointerCursorAt: (x: number, y: number) => { seen.push([x, y]); return "pointer"; },
    },
    forwardChromeEvent: () => seen.push("forward"),
    scheduleDraw() {},
  });
  const event = { pointerType: "mouse", pointerId: 1, clientX: 42, clientY: 17,
    preventDefault() {}, stopPropagation() {} } as PointerEvent;
  return { panel, seen, event };
}

test("modal overlay hover forwards first then uses Rust cursor classification", () => {
  const { panel, seen, event } = fixture(true);
  panel.handlePointerMove(event);
  assert.deepEqual(seen, ["forward", [42, 17]]);
  assert.equal(panel.canvas.style.cursor, "pointer");
  assert.equal(panel.markdownLayer.style.cursor, "pointer");
});

test("picker counts zero-detail primary presses for single-select/double-navigation", () => {
  const { panel, event } = fixture(true);
  panel.wasmAdapter.fileBrowserActive = () => true;
  assert.equal(panel.fileBrowserPointerClickCount({ ...event, button: 0, detail: 0, timeStamp: 100 }), 1);
  assert.equal(panel.fileBrowserPointerClickCount({ ...event, button: 0, detail: 0, timeStamp: 200 }), 2);
  assert.equal(panel.fileBrowserPointerClickCount({ ...event, button: 0, detail: 0, timeStamp: 700 }), 1);
  panel.canvasLogicalPoint = () => ({ x: 42, y: 70 });
  assert.equal(panel.fileBrowserPointerClickCount({ ...event, button: 0, detail: 0, timeStamp: 750 }), 1);
});

test("touch pointer moves never query or change the mouse cursor", () => {
  const { panel, seen, event } = fixture(true);
  panel.handlePointerMove({ ...event, pointerType: "touch" });
  assert.deepEqual(seen, []);
  assert.equal(panel.canvas.style.cursor, "");
});

test("active tab drag keeps ownership ahead of cursor and modal hover", () => {
  const { panel, seen, event } = fixture(true);
  panel.tabGestureOwnership.owns = () => true;
  panel.updateBufferTabDrag = () => seen.push("drag");
  panel.handlePointerMove(event);
  assert.deepEqual(seen, ["drag"]);
});
