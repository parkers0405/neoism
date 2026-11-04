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

test("animation owners share a display callback and reserve its successor before host flushes", async () => {
  const display = nativeDisplay(), first = new AnimationFrameScheduler(1000 / 240), second = new AnimationFrameScheduler(1000 / 240);
  let a = 0, b = 0;
  const animate = () => { a++; queueMicrotask(() => display.order.push("host flush")); if (a < 2) first.request(animate); };
  try {
    first.request(animate); second.request(() => b++);
    await settle(); expect(display.requests).toBe(1); expect(a + b).toBe(0);
    display.frame(); expect(a).toBe(1); expect(b).toBe(1); expect(display.ticks).toBe(1);
    await settle(); expect(display.order).toEqual(["request", "request", "host flush"]);
    display.frame(); await settle();
    expect(a).toBe(2); expect(b).toBe(1); expect(display.requests).toBe(3); expect(first.pending).toBe(false);
    display.frame();await settle();
    expect(a).toBe(2);expect(b).toBe(1);expect(display.requests).toBe(3);
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
    expect(display.requests).toBe(5);
    display.frame();await settle();expect(display.requests).toBe(5);
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

test("over-budget 120fps work still yields before requesting its next native frame",async()=>{
 const display=nativeDisplay(),animation=new AnimationFrameScheduler(1000/240);
 let fired=false;
 try{
  animation.request(()=>{fired=true;},performance.now()-100,1000/120);
  await settle();expect(display.requests).toBe(0);expect(fired).toBe(false);
  await Bun.sleep(10);await settle();expect(display.requests).toBe(1);
  display.frame();expect(fired).toBe(true);
 }finally{animation.dispose();display.clock.dispose();}
});

test("yielding animation follows native frames without a fixed-rate deadline",async()=>{
 const display=nativeDisplay(),animation=new AnimationFrameScheduler(1000/240);
 const times:number[]=[];let time=0;
 const advance=()=>{times.push(time);if(times.length<3)animation.request(advance,performance.now(),1000/240,"yield-frame");};
 try{
  animation.request(advance,performance.now(),1000/240,"yield-frame");
  await settle();expect(display.requests).toBe(0);
  for(const timestamp of [4.16,8.33,15.27]){
   const before=times.length;
   await Bun.sleep(5);await settle();expect(times).toHaveLength(before);
   time=timestamp;display.frame();
  }
  expect(times).toEqual([4.16,8.33,15.27]);
 }finally{animation.dispose();display.clock.dispose();}
});

test("cancellation during a display batch skips captured work and preserves its replacement",async()=>{
 const display=nativeDisplay(),input=new AnimationFrameScheduler(4),paint=new AnimationFrameScheduler(4);const ran:string[]=[];
 try{
  input.request(()=>{ran.push('input');paint.cancel();paint.request(()=>ran.push('replacement'));});
  paint.request(()=>ran.push('stale'));await settle();display.frame();
  expect(ran).toEqual(['input']);expect(paint.pending).toBe(true);
  await settle();display.frame();expect(ran).toEqual(['input','replacement']);expect(paint.pending).toBe(false);
 }finally{input.dispose();paint.dispose();display.clock.dispose();}
});
