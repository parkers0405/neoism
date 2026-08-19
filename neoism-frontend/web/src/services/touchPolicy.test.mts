import { test } from "node:test";
import assert from "node:assert/strict";

import {
  TouchPolicy,
  LONG_PRESS_MS,
  type TouchSample,
} from "./touchPolicy.ts";
import {
  loadWasmInputPolicyModule,
  skipReason,
} from "../presence/wasmPolicyTestSupport.mts";

// The gesture state machine lives in
// `neoism-frontend/shared/src/touch_policy.rs` (its unit tests are
// canonical); this file drives the key scenarios through the wasm
// `TouchGesturePolicy` + the TS adapter, and pins the adapter's
// pre-load inert behaviour.

const wasm = await loadWasmInputPolicyModule();
const touchSkip = skipReason(wasm, "TouchGesturePolicy");
const mobileSkip = skipReason(wasm, "mobile_keyboard_inset");

const layout = { width: 800, height: 600 };

function sample(id: number, x: number, y: number, timeMs = 0): TouchSample {
  return { id, x, y, timeMs };
}

function policy(): TouchPolicy {
  return new TouchPolicy(wasm);
}

test("pre-load: adapter is inert until the bundle arrives", () => {
  const inert = new TouchPolicy(() => null);
  assert.equal(inert.isActive(), false);
  assert.deepEqual(inert.start(sample(1, 10, 10), "terminal-body"), { kind: "none" });
  assert.deepEqual(inert.move(sample(1, 60, 10), layout), { kind: "none" });
  assert.deepEqual(inert.end(sample(1, 60, 10), layout), { kind: "none" });
  assert.deepEqual(inert.tickLongPress(9_999, layout), { kind: "none" });
});

test("tap start + lift emits a simulated click at the start point", { skip: touchSkip }, () => {
  const touch = policy();
  assert.deepEqual(touch.start(sample(1, 25, 30), "terminal-body"), { kind: "none" });
  assert.equal(touch.isActive(), true);
  const end = touch.end(sample(1, 25, 30), layout);
  assert.deepEqual(end, { kind: "end-simulated-left-click", x: 25, y: 30 });
  assert.equal(touch.isActive(), false);
});

test("horizontal motion promotes to drag-select anchored at start", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 10, 10), "terminal-body");
  const promotion = touch.move(sample(1, 50, 11), layout);
  assert.deepEqual(promotion, { kind: "start-simulated-left-click", x: 10, y: 10 });
  // Re-fed motion (host contract) streams the selection endpoint.
  const extend = touch.move(sample(1, 50, 11), layout);
  assert.deepEqual(extend, { kind: "update-mouse-position", x: 50, y: 11 });
  const end = touch.end(sample(1, 60, 11), layout);
  assert.deepEqual(end, { kind: "end-select" });
});

test("vertical motion promotes to scroll with re-feed delta", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 10, 10), "terminal-body");
  assert.deepEqual(touch.move(sample(1, 11, 50), layout), {
    kind: "promote-tap-to-scroll",
  });
  const action = touch.move(sample(1, 11, 60), layout);
  assert.equal(action.kind, "scroll");
  if (action.kind === "scroll") {
    assert.equal(action.dx, 0);
    assert.ok(action.dy > 0);
  }
  assert.deepEqual(touch.end(sample(1, 11, 60), layout), { kind: "end-scroll" });
});

test("editor-area horizontal motion scrolls instead of selecting", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 10, 10), "editor-area");
  assert.deepEqual(touch.move(sample(1, 23, 10), layout), { kind: "none" });
  assert.deepEqual(touch.move(sample(1, 28, 10), layout), {
    kind: "promote-tap-to-scroll",
  });
});

test("pinch on terminal body changes font size; chrome panel suppresses", { skip: touchSkip }, () => {
  const body = policy();
  body.start(sample(1, 100, 10), "terminal-body");
  body.start(sample(2, 200, 10), "terminal-body");
  assert.deepEqual(body.move(sample(2, 500, 10), layout), {
    kind: "change-font-size",
    direction: "increase",
  });

  const panel = policy();
  panel.start(sample(1, 100, 10), "chrome-panel");
  panel.start(sample(2, 200, 10), "chrome-panel");
  assert.deepEqual(panel.move(sample(2, 500, 10), layout), {
    kind: "suppress-native-gesture",
  });
});

test("two-finger same-direction pan scrolls instead of zooming", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 100, 100), "terminal-body");
  touch.start(sample(2, 200, 100), "terminal-body");
  const commit = touch.move(sample(2, 200, 112), layout);
  assert.equal(commit.kind, "two-finger-scroll");
  const next = touch.move(sample(2, 200, 130), layout);
  assert.equal(next.kind, "two-finger-scroll");
  if (next.kind === "two-finger-scroll") {
    assert.equal(next.dx, 0);
    assert.ok(next.dy > 0);
  }
});

test("long-press promotes to context menu exactly once", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 40, 50, 1_000), "terminal-body");
  assert.deepEqual(touch.tickLongPress(1_100, layout), { kind: "none" });
  assert.deepEqual(touch.tickLongPress(1_000 + LONG_PRESS_MS + 1, layout), {
    kind: "open-context-menu",
    x: 40,
    y: 50,
  });
  assert.deepEqual(touch.tickLongPress(1_000 + LONG_PRESS_MS + 50, layout), {
    kind: "none",
  });
  // The lift after a long-press must not double-fire a click.
  assert.deepEqual(touch.end(sample(1, 40, 50), layout), { kind: "none" });
});

test("swipe-back suppression matches the shared rule", { skip: touchSkip }, () => {
  assert.equal(wasm!.touch_should_suppress_swipe_back!("editor-area"), true);
  assert.equal(wasm!.touch_should_suppress_swipe_back!("terminal-body"), false);
  assert.equal(wasm!.touch_should_suppress_swipe_back!("chrome-panel"), false);
  // Static adapter fallback agrees.
  assert.equal(TouchPolicy.shouldSuppressSwipeBack("editor-area"), true);
  assert.equal(TouchPolicy.shouldSuppressSwipeBack("terminal-body"), false);
});

test("host takeover resets the gesture", { skip: touchSkip }, () => {
  const touch = policy();
  touch.start(sample(1, 10, 10), "terminal-body");
  assert.equal(touch.isActive(), true);
  touch.reset();
  assert.equal(touch.isActive(), false);
  assert.deepEqual(touch.end(sample(1, 10, 10), layout), { kind: "none" });
});

// ── Mobile soft-keyboard policy exports ─────────────────────────────

test("keyboard inset math matches the shared policy", { skip: mobileSkip }, () => {
  const open = wasm!.mobile_keyboard_inset!(800, 500, 0);
  assert.deepEqual(open, { bottom: 300, keyboardOpen: true });
  const slop = wasm!.mobile_keyboard_inset!(800, 797.4, 0);
  assert.deepEqual(slop, { bottom: 3, keyboardOpen: false });
  const clamped = wasm!.mobile_keyboard_inset!(800, 900, 0);
  assert.deepEqual(clamped, { bottom: 0, keyboardOpen: false });
});

test("mobile input attributes match the capture-element contract", { skip: mobileSkip }, () => {
  assert.deepEqual(wasm!.mobile_input_attributes!("code", false), {
    autocapitalize: "off",
    autocorrect: "off",
    spellcheck: "false",
    inputmode: "text",
    enterkeyhint: "send",
  });
  assert.equal(wasm!.mobile_input_attributes!("editor", false)?.enterkeyhint, "enter");
  assert.equal(wasm!.mobile_input_attributes!("search", false)?.inputmode, "search");
  assert.equal(wasm!.mobile_input_attributes!("url", true)?.inputmode, "none");
  assert.equal(wasm!.mobile_input_attributes!("text", false)?.autocorrect, "on");
});

test("mobile key byte tables match the toolbar contract", { skip: mobileSkip }, () => {
  assert.deepEqual(
    Array.from(wasm!.mobile_named_key_bytes!("ArrowUp") ?? []),
    [0x1b, 0x5b, 0x41],
  );
  assert.deepEqual(Array.from(wasm!.mobile_named_key_bytes!("Enter") ?? []), [0x0d]);
  assert.deepEqual(Array.from(wasm!.mobile_named_key_bytes!("Backspace") ?? []), [0x7f]);
  assert.equal(wasm!.mobile_named_key_bytes!("F13"), undefined);
  assert.equal(wasm!.mobile_ctrl_chord_byte!("c"), 3);
  assert.equal(wasm!.mobile_ctrl_chord_byte!("C"), 3);
  assert.equal(wasm!.mobile_ctrl_chord_byte!("1"), undefined);
});
