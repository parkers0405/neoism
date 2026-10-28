import type { GpuixRenderer, EventPayload } from "@gpuix/native";
// Same 500ms buckets / 1s idle-burst reset as desktop/host/fps.rs.
// Observe actual native paint counters; never request an extra frame. Rates
// divide draw intervals by the native draw clock: polling time would let a
// bucket catch one frame late and the next one early, reading above vsync.
class FrameMeter {
  previous = 0; last = 0; start = 0; count = 0; value?: number;
  listeners = new Set<() => void>();
  /** `now` is the draw time of the `frames`-th draw when the native clock is available. */
  observe(frames: number, now: number) {
    if (!Number.isFinite(frames) || frames <= this.previous) return;
    const delta = frames - this.previous; this.previous = frames;
    if (!this.last || now - this.last > 1000) { this.start = now; this.count = 0; }
    else this.count += delta;
    this.last = now;
    const elapsed = now - this.start;
    if (elapsed >= 500) { this.value = Math.max(1, Math.round(this.count * 1000 / elapsed)); this.start = now; this.count = 0; }
  }
}
const key = Symbol.for("neoism/gpuix/frame-meters");
const globals = globalThis as typeof globalThis & { [key]?: WeakMap<object, FrameMeter> };
const meters = globals[key] ||= new WeakMap<object, FrameMeter>();
export function installFrameMeter(renderer: GpuixRenderer, clock = () => performance.now()) {
  if (meters.has(renderer)) return;
  const meter = new FrameMeter(); meters.set(renderer, meter);
  const tick = renderer.tick.bind(renderer);
  let nextSample = -Infinity;
  renderer.tick = () => {
    const running = tick();
    // The draw counter is a lock-free native atomic. The debug-stats getter it
    // replaces waited for the UI thread, parking JavaScript behind a paint and
    // costing frames. The pill still publishes half-second buckets.
    if (running !== false && clock() >= nextSample) {
      const native = renderer as GpuixRenderer & { getRenderedFrameSample?: () => number[] };
      const now = clock(); nextSample = now + 100;
      if (native.getRenderedFrameSample) { const [frames, drawnAt] = native.getRenderedFrameSample(); meter.observe(frames, drawnAt); }
      else meter.observe(renderer.getDebugFrameOverlayStats().frames, now);
    }
    return running;
  };
}
export function measuredFrameRate(renderer: object) { return meters.get(renderer)?.value; }
export function onFrameActivity(renderer: object, listener: () => void) {
  const meter = meters.get(renderer); meter?.listeners.add(listener);
  return () => { meter?.listeners.delete(listener); };
}
export function notifyFrameActivity(renderer: object, event: EventPayload) {
  // Refresh the display during user input that already owns redraw work.
  // Layout, timers and the FPS display itself cannot keep a window repainting.
  if (["scroll", "mouseDown", "keyDown", "click"].includes(event.eventType)) for (const listener of meters.get(renderer)?.listeners || []) listener();
}
