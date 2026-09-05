import { test } from "node:test";
import assert from "node:assert/strict";

import { TabGestureOwnership } from "./tabGestureOwnership.ts";

const tabRect = { x: 20, y: 36, w: 180, h: 32 };
const overlaps = (x: number, y: number) =>
  x >= tabRect.x && x <= tabRect.x + tabRect.w &&
  y >= tabRect.y && y <= tabRect.y + tabRect.h;

test("tab-owned pointer click cannot place a caret after activating code", () => {
  const ownership = new TabGestureOwnership();
  let activeSurface: "terminal" | "code" = "terminal";
  const selection = { anchor: 41, head: 47 };
  let editorDown = 0;
  let editorUp = 0;
  const placeCaret = () => {
    editorDown += 1;
    selection.anchor = 3;
    selection.head = 3;
  };
  // This is tab 2 before activation and a code-line coordinate afterwards.
  const point = { x: 96, y: 52 };

  if (overlaps(point.x, point.y)) {
    ownership.claim(7, "workspace-tabs");
    activeSurface = "code";
  } else if ((activeSurface as string) === "code") {
    placeCaret();
  }
  // Browser pointerup occurs after the activation/render boundary.
  if (ownership.owns(7)) ownership.release(7);
  else if ((activeSurface as string) === "code") editorUp += 1;

  assert.deepEqual(selection, { anchor: 41, head: 47 });
  assert.equal(editorDown, 0);
  assert.equal(editorUp, 0);
});

test("touch simulated tab click remains chrome-only through release", () => {
  const ownership = new TabGestureOwnership();
  const selection = { anchor: 9, head: 12 };
  let contentDispatches = 0;
  const touch = { id: 23, x: 96, y: 52 };

  ownership.claim(touch.id, "workspace-tabs");
  // touchend dispatches its simulated down/up directly to shared Chrome.
  if (!ownership.owns(touch.id)) {
    contentDispatches += 1;
    selection.anchor = 2;
    selection.head = 2;
  }
  ownership.release(touch.id);

  assert.deepEqual(selection, { anchor: 9, head: 12 });
  assert.equal(contentDispatches, 0);
});

test("pane-tab ownership survives move until cancel", () => {
  const ownership = new TabGestureOwnership();
  ownership.claim(4, "pane-tabs");
  assert.equal(ownership.source(4), "pane-tabs");
  assert.equal(ownership.release(4), "pane-tabs");
  assert.equal(ownership.owns(4), false);
});