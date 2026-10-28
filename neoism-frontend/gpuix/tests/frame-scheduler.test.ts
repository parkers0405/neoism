import { expect, test } from "bun:test";
import { animationDelay, AnimationFrameScheduler } from "../src/animation/frame-scheduler";

function clock(intervalMs = 1000 / 240) {
  let now = 0, next = 0;
  const timers = new Map<number, { callback: () => void; at: number }>();
  const scheduler = new AnimationFrameScheduler(intervalMs, () => now, (callback, delay) => {
    const id = ++next; timers.set(id, { callback, at: now + delay });
    return () => { timers.delete(id); };
  });
  return { scheduler, timers, advance(milliseconds: number) { now += milliseconds; },
    fire() { const [id, timer] = [...timers][0]; timers.delete(id); now = Math.max(now, timer.at); timer.callback(); } };
}

test("animation budgets include work already spent and permit display rates above120Hz", () => {
  const interval = 1000 / 240;
  expect(animationDelay(100, interval, 103)).toBeCloseTo(interval - 3);
  expect(animationDelay(100, interval, 120)).toBe(0);
  expect(animationDelay(100, 1000 / 144, 102)).toBeCloseTo(1000 / 144 - 2);
});

test("one animation owner coalesces requests and stops entirely when work finishes", () => {
  const { scheduler, timers, advance, fire } = clock();
  let frames = 0;
  advance(3); scheduler.request(() => { frames++; }, 0);
  scheduler.request(() => { throw new Error("Duplicate frame"); });
  expect(timers.size).toBe(1); expect([...timers.values()][0].at).toBeCloseTo(1000 / 240);
  fire(); expect(frames).toBe(1); expect(scheduler.pending).toBe(false); expect(timers.size).toBe(0);
});

test("continuing animation owns one next timer and unmount cancels all future work", () => {
  const { scheduler, timers, fire } = clock();
  let frames = 0;
  const animate = () => { frames++; scheduler.request(animate); };
  scheduler.request(animate); fire();
  expect(frames).toBe(1); expect(timers.size).toBe(1);
  scheduler.cancel(); expect(timers.size).toBe(0); expect(scheduler.pending).toBe(false);
  scheduler.request(animate); expect(timers.size).toBe(1);
  scheduler.dispose(); scheduler.request(animate);
  expect(timers.size).toBe(0); expect(frames).toBe(1);
});

test("native deadlines can request an earlier wake than the animation interval", () => {
  const { scheduler, timers, advance, fire } = clock();
  let fired = false;
  advance(1); scheduler.request(() => { fired = true; }, 0, 2);
  expect([...timers.values()][0].at).toBe(2);
  fire(); expect(fired).toBe(true); expect(timers.size).toBe(0);
});
