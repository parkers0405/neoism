import assert from "node:assert/strict";
import test from "node:test";

import {
  DirectTouchScrollGesture,
  TouchMomentum,
  TOUCH_SCROLL_INTENT_PX,
  normalizedTouchEventTime,
} from "./directTouchScroll.ts";

test("threshold promotion re-feeds the displacement instead of dropping it", () => {
  const gesture = new DirectTouchScrollGesture("y", 20, 100);
  assert.deepEqual(gesture.update(21, 100 + TOUCH_SCROLL_INTENT_PX), {
    moved: false,
    scrolling: false,
    delta: 0,
  });
  assert.deepEqual(gesture.update(21, 107), {
    moved: true,
    scrolling: true,
    delta: 7,
  });
});

test("sparse release keeps recent velocity and ignores stationary touchend", () => {
  const momentum = new TouchMomentum();
  momentum.begin(0, 100, 0);
  momentum.sampleBatch([
    { x: 0, y: 85, timeMs: 30 },
    { x: 0, y: 50, timeMs: 70 },
  ]);
  momentum.sample(0, 50, 145); // stationary final event must not snub launch
  assert.equal(momentum.release(170, "y"), true);
  assert.ok(momentum.step(186).dy < 0);
});

test("event timestamps normalize epoch WebKit samples into performance time", () => {
  const now = 2500;
  assert.equal(normalizedTouchEventTime(2488, now), 2488);
  const epoch = performance.timeOrigin + 2490;
  assert.ok(Math.abs(normalizedTouchEventTime(epoch, now) - 2490) < 1);
  assert.equal(normalizedTouchEventTime(Number.NaN, now), now);
});

test("committed scrolling emits exact 1:1 incremental deltas", () => {
  const gesture = new DirectTouchScrollGesture("y", 0, 50);
  assert.equal(gesture.update(0, 44).delta, -6);
  assert.equal(gesture.update(0, 35).delta, -9);
  assert.equal(gesture.update(0, 38).delta, 3);
  assert.equal(gesture.isScrolling(), true);
});

test("movement suppresses click even when the cross axis wins", () => {
  const tabs = new DirectTouchScrollGesture("x", 10, 10);
  const update = tabs.update(11, 18);
  assert.equal(update.scrolling, false);
  assert.equal(update.moved, true);
  assert.equal(tabs.didMove(), true);
});

test("the same policy serves horizontal tabs and vertical tree surfaces", () => {
  const tabs = new DirectTouchScrollGesture("x", 100, 20);
  const tree = new DirectTouchScrollGesture("y", 20, 100);
  assert.equal(tabs.update(91, 21).delta, -9);
  assert.equal(tree.update(21, 112).delta, 12);
});

test("recent acceleration launches proportionally faster momentum", () => {
  const steady = new TouchMomentum();
  steady.begin(0, 0, 0);
  steady.sample(0, 10, 20);
  steady.sample(0, 20, 40);
  assert.equal(steady.release(40, "y"), true);
  const steadyFrame = steady.step(56).dy;

  const accelerating = new TouchMomentum();
  accelerating.begin(0, 0, 0);
  accelerating.sample(0, 4, 20);
  accelerating.sample(0, 20, 40);
  assert.equal(accelerating.release(40, "y"), true);
  assert.ok(accelerating.step(56).dy > steadyFrame);
});

test("momentum decays smoothly and stops without a low-speed crawl", () => {
  const momentum = new TouchMomentum();
  momentum.begin(0, 0, 0);
  momentum.sample(0, 40, 20);
  assert.equal(momentum.release(20, "y"), true);
  const deltas: number[] = [];
  let now = 20;
  while (momentum.isRunning() && deltas.length < 200) {
    now += 16;
    deltas.push(momentum.step(now).dy);
  }
  assert.ok(deltas.length > 2);
  assert.ok(deltas.length < 90, "firm velocity floor prevents a long crawl");
  assert.ok(deltas.every((value, i) => i === 0 || value < deltas[i - 1]));
  assert.equal(momentum.isRunning(), false);
});

test("axis lock, bounds cancellation, and stop-touch click suppression", () => {
  const momentum = new TouchMomentum();
  momentum.begin(0, 0, 0);
  momentum.sample(30, 5, 20);
  assert.equal(momentum.release(20, "x"), true);
  const frame = momentum.step(36);
  assert.equal(frame.dy, 0);
  // A surface reports a hard bound by cancelling the shared policy. The
  // return value tells the host to suppress the stop-touch's click.
  assert.equal(momentum.cancel(), true);
  assert.equal(momentum.cancel(), false);
  assert.equal(momentum.step(52).active, false);
});