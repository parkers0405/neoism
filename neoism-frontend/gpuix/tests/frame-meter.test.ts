import { expect, test } from "bun:test";
import type { GpuixRenderer } from "@gpuix/native";
import { installFrameMeter, measuredFrameRate } from "../src/chrome/frame-meter";

function fixture() {
  let now = 1, frames = 0, samples = 0, running = true;
  const renderer = {
    tick: () => running,
    getDebugFrameOverlayStats: () => { samples++; return { frames }; },
  } as unknown as GpuixRenderer;
  const clock = () => now;
  installFrameMeter(renderer, clock);
  return { renderer, clock, get samples() { return samples; },
    at(time: number, count: number) { now = time; frames = count; renderer.tick(); },
    stop() { running = false; now += 1000; return renderer.tick(); } };
}

test("FPS measurement counts all native draws with at most ten UI-thread samples per second", () => {
  const f = fixture();
  for (let elapsed = 0; elapsed <= 1000; elapsed += 8) f.at(1 + elapsed, Math.floor(elapsed * 144 / 1000));
  expect(f.samples).toBeLessThanOrEqual(10);
  expect(measuredFrameRate(f.renderer)).toBeGreaterThanOrEqual(143);
  expect(measuredFrameRate(f.renderer)).toBeLessThanOrEqual(145);
  // Re-installing during hot replacement must not wrap/poll a second time.
  installFrameMeter(f.renderer, f.clock);
  const samples = f.samples; f.at(1010, 145);
  expect(f.samples).toBe(samples);
});

test("idle gaps retain the last FPS bucket and a new burst measures fresh draws", () => {
  const f = fixture();
  for (let i = 0; i <= 10; i++) f.at(1 + i * 100, i * 14);
  const previous = measuredFrameRate(f.renderer);
  f.at(5001, 141);
  expect(measuredFrameRate(f.renderer)).toBe(previous);
  for (let i = 1; i <= 5; i++) f.at(5001 + i * 100, 141 + i * 6);
  expect(measuredFrameRate(f.renderer)).toBe(60);
});

test("a terminated window never receives another synchronous stats request", () => {
  const f = fixture(); f.at(1, 1);
  expect(f.samples).toBe(1);
  expect(f.stop()).toBe(false);
  expect(f.samples).toBe(1);
});

test("Wayland FPS counts displayed frames rather than faster submitted draws",()=>{
 let now=1,frames=0;
 const renderer={tick:()=>true,getPresentedFrameSample:()=>[frames,now],getRenderedFrameSample:()=>{throw new Error("must use compositor feedback");}} as unknown as GpuixRenderer;
 installFrameMeter(renderer,()=>now);
 for(let i=0;i<=12;i++){frames=i*12;now=1+frames*1000/144;renderer.tick();}
 expect(measuredFrameRate(renderer)).toBe(144);
});
