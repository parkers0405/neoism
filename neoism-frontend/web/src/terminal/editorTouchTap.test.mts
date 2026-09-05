import assert from "node:assert/strict";
import test from "node:test";

import { routeEditorTouchTap } from "./editorTouchTap.ts";

test("resolved mobile editor tap sends down/up with tap geometry", () => {
  const calls: unknown[][] = [];
  const handled = routeEditorTouchTap(
    {
      editorPointerDown: (...args) => {
        calls.push(["down", ...args]);
        return true;
      },
      editorPointerUp: () => {
        calls.push(["up"]);
        return true;
      },
    },
    true,
    123,
    456,
    () => calls.push(["effects"]),
  );

  assert.equal(handled, true);
  assert.deepEqual(calls, [
    ["down", 123, 456, false, false, 1],
    ["up"],
    ["effects"],
  ]);
});

test("inactive editor and drag-owned paths send no editor pointer calls", () => {
  let calls = 0;
  const adapter = {
    editorPointerDown: () => {
      calls += 1;
      return true;
    },
    editorPointerUp: () => {
      calls += 1;
      return true;
    },
  };

  // TerminalPanel invokes this router only for end-simulated-left-click.
  // A drag resolves to end-scroll and therefore makes no call at all.
  assert.equal(routeEditorTouchTap(adapter, false, 10, 20, () => { calls += 1; }), false);
  assert.equal(calls, 0);
});