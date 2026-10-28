import { expect, test } from "bun:test";
import { installDisplayClock, type DisplayRenderer } from "../src/animation/display-clock";
import { AnimationFrameScheduler } from "../src/animation/frame-scheduler";

const settle = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
function nativeDisplay() {
  let callback: (() => void) | null | undefined, requests = 0, ticks = 0;
  const order: string[] = [];
  const renderer: DisplayRenderer = {
    setFrameWakeCallback(next) { callback = next; return true; },
    requestFrameWake() { requests++; order.push("request"); return true; },
  };
  const errors: unknown[] = [], clock = installDisplayClock(renderer, error => errors.push(error));
  renderer.setFrameWakeCallback!(() => { ticks++; });
  return { renderer, clock, order, errors, get requests() { return requests; }, get ticks() { return ticks; }, frame() { callback?.(); } };
}

test("animation owners share the native display callback and submit after queued host mutations", async () => {
  const display = nativeDisplay(), first = new AnimationFrameScheduler(1000 / 240), second = new AnimationFrameScheduler(1000 / 240);
  let a = 0, b = 0;
  const animate = () => { a++; queueMicrotask(() => display.order.push("host flush")); if (a < 2) first.request(animate); };
  try {
    first.request(animate); second.request(() => b++);
    await settle(); expect(display.requests).toBe(1); expect(a + b).toBe(0);
    display.frame(); expect(a).toBe(1); expect(b).toBe(1); expect(display.ticks).toBe(1);
    await settle(); expect(display.order).toEqual(["request", "host flush", "request"]);
    display.frame(); await settle();
    expect(a).toBe(2); expect(b).toBe(1); expect(display.requests).toBe(2); expect(first.pending).toBe(false);
    expect(display.errors).toEqual([]);
  } finally { first.dispose(); second.dispose(); display.clock.dispose(); }
});

test("cancelled and completed animations leave no requested frame loop behind", async () => {
  const display = nativeDisplay(), animation = new AnimationFrameScheduler(1000 / 240);
  let frames = 0;
  try {
    animation.request(() => frames++); animation.cancel(); await settle();
    expect(display.requests).toBe(0);
    animation.request(() => frames++); await settle(); expect(display.requests).toBe(1);
    animation.dispose(); display.frame(); await settle();
    expect(frames).toBe(0); expect(display.requests).toBe(1);
  } finally { animation.dispose(); display.clock.dispose(); }
});

test("refresh-rate changes control animation advancement through native callbacks", async () => {
  const display = nativeDisplay(), animation = new AnimationFrameScheduler(1000 / 240);
  let time = 0; const observed: number[] = [];
  const animate = () => { observed.push(time); if (observed.length < 4) animation.request(animate); };
  try {
    animation.request(animate); await settle();
    for (const timestamp of [6.94, 13.89, 30.56, 47.22]) { time = timestamp; display.frame(); await settle(); }
    expect(observed).toEqual([6.94, 13.89, 30.56, 47.22]);
    expect(display.requests).toBe(4);
  } finally { animation.dispose(); display.clock.dispose(); }
});

test("real deadlines sleep first and still deliver work only on a native frame", async () => {
  const display = nativeDisplay(), animation = new AnimationFrameScheduler(1);
  let fired = false;
  try {
    animation.request(() => { fired = true; }, performance.now(), 10);
    await settle(); expect(display.requests).toBe(0);
    await Bun.sleep(20); await settle(); expect(display.requests).toBe(1); expect(fired).toBe(false);
    display.frame(); expect(fired).toBe(true);
  } finally { animation.dispose(); display.clock.dispose(); }
});
